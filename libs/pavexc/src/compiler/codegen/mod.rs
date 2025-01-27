use std::collections::BTreeMap;

use ahash::HashMap;
use bimap::BiHashMap;
use cargo_manifest::{Dependency, DependencyDetail, Edition};
use deps::ServerSdkDeps;
use guppy::graph::{ExternalSource, PackageSource};
use guppy::PackageId;
use indexmap::{IndexMap, IndexSet};
use proc_macro2::{Ident, TokenStream};
use quote::{format_ident, quote};
use router::codegen_router;
use syn::{ItemEnum, ItemFn, ItemStruct};

use crate::compiler::analyses::call_graph::{
    ApplicationStateCallGraph, CallGraphNode, RawCallGraph,
};
use crate::compiler::analyses::components::{ComponentDb, ComponentId};
use crate::compiler::analyses::computations::ComputationDb;
use crate::compiler::analyses::framework_items::FrameworkItemDb;
use crate::compiler::analyses::processing_pipeline::RequestHandlerPipeline;
use crate::compiler::analyses::router::Router;
use crate::compiler::app::GENERATED_APP_PACKAGE_ID;
use crate::compiler::computation::Computation;
use crate::language::{Callable, GenericArgument, ResolvedType};
use crate::rustdoc::{ALLOC_PACKAGE_ID_REPR, TOOLCHAIN_CRATES};

use super::analyses::application_state::ApplicationState;
use super::generated_app::GeneratedManifest;

mod deps;
mod router;

pub(crate) fn codegen_app(
    router: &Router,
    handler_id2pipeline: &IndexMap<ComponentId, RequestHandlerPipeline>,
    application_state_call_graph: &ApplicationStateCallGraph,
    request_scoped_framework_bindings: &BiHashMap<Ident, ResolvedType>,
    package_id2name: &BiHashMap<PackageId, String>,
    application_state: &ApplicationState,
    codegen_deps: &HashMap<String, PackageId>,
    component_db: &ComponentDb,
    computation_db: &ComputationDb,
    framework_item_db: &FrameworkItemDb,
) -> Result<TokenStream, anyhow::Error> {
    let sdk_deps = ServerSdkDeps::new(codegen_deps, package_id2name);
    let application_state_def = define_application_state(application_state, package_id2name);
    if tracing::event_enabled!(tracing::Level::TRACE) {
        eprintln!(
            "Application state definition:\n{}",
            quote! { #application_state_def }
        );
    }
    let define_application_state_error = define_application_state_error(
        &application_state_call_graph.error_variants,
        package_id2name,
        &sdk_deps,
    );
    let application_state_init = get_application_state_init(
        application_state_call_graph,
        package_id2name,
        component_db,
        computation_db,
    )?;

    let define_server_state = define_server_state(&application_state_def);

    let route_infos = router.route_infos();
    let handler_id2codegened_pipeline = handler_id2pipeline
        .iter()
        .map(|(id, p)| {
            let span = tracing::info_span!("Codegen request handler pipeline", route_info = %route_infos[*id]);
            let _guard = span.enter();
            p.codegen(
                sdk_deps.pavex_ident(),
                package_id2name,
                component_db,
                computation_db,
            )
            .map(|p| (*id, p))
        })
        .collect::<Result<BTreeMap<_, _>, _>>()?;
    let handler_modules = handler_id2codegened_pipeline
        .values()
        .map(|p| p.as_inline_module())
        .collect::<Vec<_>>();

    let entrypoint = server_startup(&sdk_deps);
    let alloc_extern_import = if package_id2name.contains_right(ALLOC_PACKAGE_ID_REPR) {
        // The fact that an item from `alloc` is used in the generated code does not imply
        // that we need to have an `alloc` import (e.g. it might not appear in function
        // signatures).
        // Nonetheless, we add the import to be on the safe side.
        // See https://doc.rust-lang.org/edition-guide/rust-2018/path-changes.html#an-exception
        // for an explanation of why we need the "extern crate" syntax here.
        quote! {
            extern crate alloc;
        }
    } else {
        quote! {}
    };
    let router = codegen_router(
        router,
        &sdk_deps,
        &handler_id2codegened_pipeline,
        application_state,
        request_scoped_framework_bindings,
        package_id2name,
        framework_item_db,
    );
    let code = quote! {
        //! Do NOT edit this code.
        //! It was automatically generated by Pavex.
        //! All manual edits will be lost next time the code is generated.
        #alloc_extern_import
        #define_server_state
        #application_state_def
        #define_application_state_error
        #application_state_init
        #entrypoint
        #router
        #(#handler_modules)*
    };
    Ok(code)
}

fn server_startup(sdk_deps: &ServerSdkDeps) -> ItemFn {
    let pavex = sdk_deps.pavex_ident();
    let http = sdk_deps.http_ident();
    let hyper = sdk_deps.hyper_ident();
    syn::parse2(quote! {
        pub fn run(
            server_builder: #pavex::server::Server,
            application_state: ApplicationState
        ) -> #pavex::server::ServerHandle {
            // A little bit of boilerplate to make the handler signature match the one expected by
            // `ServerBuilder::serve`.
            async fn handler(
                request: #http::Request<#hyper::body::Incoming>,
                connection_info: Option<#pavex::connection::ConnectionInfo>,
                server_state: std::sync::Arc<ServerState>
            ) -> #pavex::response::Response {
                let (router, state) = (&server_state.router, &server_state.application_state);
                router.route(request, connection_info, state).await
            }

            let router = Router::new();
            let server_state = std::sync::Arc::new(ServerState {
                router,
                application_state
            });

            server_builder.serve(handler, server_state)
        }
    })
    .unwrap()
}

fn define_application_state(
    application_state: &ApplicationState,
    package_id2name: &BiHashMap<PackageId, String>,
) -> ItemStruct {
    let bindings = application_state
        .bindings()
        .iter()
        .map(|(field_name, type_)| {
            let field_type = type_.syn_type(package_id2name);
            (field_name, field_type)
        })
        .collect::<BTreeMap<_, _>>();

    let fields = bindings.iter().map(|(field_name, type_)| {
        quote! { pub #field_name: #type_ }
    });
    syn::parse2(quote! {
        pub struct ApplicationState {
            #(#fields),*
        }
    })
    .unwrap()
}

fn define_application_state_error(
    error_types: &IndexMap<String, ResolvedType>,
    package_id2name: &BiHashMap<PackageId, String>,
    sdk_deps: &ServerSdkDeps,
) -> Option<ItemEnum> {
    let thiserror = sdk_deps.thiserror_ident();
    if error_types.is_empty() {
        return None;
    }
    let singleton_fields = error_types.iter().map(|(variant_name, type_)| {
        let variant_type = type_.syn_type(package_id2name);
        let variant_name = format_ident!("{}", variant_name);
        quote! {
            #[error(transparent)]
            #variant_name(#variant_type)
        }
    });
    Some(
        syn::parse2(quote! {
            #[derive(Debug, #thiserror::Error)]
            pub enum ApplicationStateError {
                #(#singleton_fields),*
            }
        })
        .unwrap(),
    )
}

fn define_server_state(application_state_def: &ItemStruct) -> ItemStruct {
    let dead_code = if application_state_def.fields.is_empty() {
        quote! {
            #[allow(dead_code)]
        }
    } else {
        quote! {}
    };
    syn::parse2(quote! {
        struct ServerState {
            router: Router,
            #dead_code
            application_state: ApplicationState
        }
    })
    .unwrap()
}

#[tracing::instrument("Codegen application state initialization function", skip_all)]
fn get_application_state_init(
    application_state_call_graph: &ApplicationStateCallGraph,
    package_id2name: &BiHashMap<PackageId, String>,
    component_db: &ComponentDb,
    computation_db: &ComputationDb,
) -> Result<ItemFn, anyhow::Error> {
    let mut function = application_state_call_graph.call_graph.codegen(
        package_id2name,
        component_db,
        computation_db,
    )?;
    function.sig.ident = format_ident!("build_application_state");
    if !application_state_call_graph.error_variants.is_empty() {
        function.sig.output = syn::ReturnType::Type(
            Default::default(),
            Box::new(syn::parse2(
                quote! { Result<crate::ApplicationState, crate::ApplicationStateError> },
            )?),
        );
    }
    Ok(function)
}

pub(crate) fn codegen_manifest<'a, I>(
    package_graph: &guppy::graph::PackageGraph,
    handler_call_graphs: I,
    application_state_call_graph: &'a RawCallGraph,
    request_scoped_framework_bindings: &'a BiHashMap<Ident, ResolvedType>,
    codegen_deps: &'a HashMap<String, PackageId>,
    component_db: &'a ComponentDb,
    computation_db: &'a ComputationDb,
) -> (GeneratedManifest, BiHashMap<PackageId, String>)
where
    I: Iterator<Item = &'a RequestHandlerPipeline>,
{
    let (dependencies, mut package_ids2deps) = compute_dependencies(
        package_graph,
        handler_call_graphs,
        application_state_call_graph,
        request_scoped_framework_bindings,
        codegen_deps,
        component_db,
        computation_db,
    );
    let manifest = GeneratedManifest {
        dependencies,
        edition: Edition::E2021,
    };

    // Toolchain crates are not listed as dependencies in the manifest, but we need to add them to
    // the package_ids2deps map so that we can generate the correct import statements.
    let toolchain_package_ids = TOOLCHAIN_CRATES
        .iter()
        .map(|p| PackageId::new(*p))
        .collect::<Vec<_>>();
    for package_id in &toolchain_package_ids {
        package_ids2deps.insert(package_id.clone(), package_id.repr().into());
    }

    // Same for the generated app package: local items can be imported using the `crate` shortcut.
    let generated_app_package_id = PackageId::new(GENERATED_APP_PACKAGE_ID);
    package_ids2deps.insert(generated_app_package_id, "crate".into());

    (manifest, package_ids2deps)
}

fn compute_dependencies<'a, I>(
    package_graph: &guppy::graph::PackageGraph,
    handler_pipelines: I,
    application_state_call_graph: &'a RawCallGraph,
    request_scoped_framework_bindings: &'a BiHashMap<Ident, ResolvedType>,
    codegen_deps: &'a HashMap<String, PackageId>,
    component_db: &'a ComponentDb,
    computation_db: &'a ComputationDb,
) -> (BTreeMap<String, Dependency>, BiHashMap<PackageId, String>)
where
    I: Iterator<Item = &'a RequestHandlerPipeline>,
{
    let package_ids = collect_package_ids(
        handler_pipelines,
        application_state_call_graph,
        request_scoped_framework_bindings,
        codegen_deps,
        component_db,
        computation_db,
    );
    let mut external_crates: IndexMap<&str, IndexSet<PackageId>> = Default::default();
    let workspace_root = package_graph.workspace().root();
    for package_id in &package_ids {
        if package_id.repr() != GENERATED_APP_PACKAGE_ID
            && !TOOLCHAIN_CRATES.contains(&package_id.repr())
        {
            let metadata = package_graph.metadata(package_id).unwrap();
            external_crates
                .entry(metadata.name())
                .or_default()
                .insert(package_id.to_owned());
        }
    }
    let mut dependencies = BTreeMap::new();
    let mut package_ids2dependency_name = BiHashMap::new();
    for (name, entries) in external_crates {
        let needs_rename = entries.len() > 1;
        for package_id in &entries {
            let metadata = package_graph.metadata(package_id).unwrap();
            let version = metadata.version();
            let mut dependency_details = DependencyDetail {
                version: Some(version.to_string()),
                // We disable default features to avoid enabling by mistake
                // features that were explicitly disabled in the app manifest.
                // This is a conservative choice, but it's better to be safe than sorry.
                // We can use a more fine-grained approach in the future if needed, e.g. by
                // analyzing which features are actually used in the code.
                // Until then, we rely on feature unification to ensure everything works as expected
                // in the final binary.
                default_features: Some(false),
                ..DependencyDetail::default()
            };
            if needs_rename {
                dependency_details.package = Some(name.to_string());
            }

            let source = metadata.source();
            match source {
                PackageSource::Workspace(p) | PackageSource::Path(p) => {
                    let path = if p.is_relative() {
                        workspace_root.join(p)
                    } else {
                        p.to_owned()
                    };
                    dependency_details.path = Some(path.to_string());
                }
                PackageSource::External(_) => {
                    if let Some(parsed_external) = source.parse_external() {
                        match parsed_external {
                            ExternalSource::Registry(registry) => {
                                if registry != ExternalSource::CRATES_IO_URL {
                                    // TODO: this is unlikely to work as is, because the `Cargo.toml` should contain
                                    //   the registry alias, not the raw registry URL.
                                    //   We can retrieve the alias from the .cargo/config.toml (probably).
                                    dependency_details.registry = Some(registry.to_string());
                                }
                            }
                            ExternalSource::Git {
                                repository, req, ..
                            } => {
                                dependency_details.git = Some(repository.to_string());
                                match req {
                                    guppy::graph::GitReq::Branch(branch) => {
                                        dependency_details.branch = Some(branch.to_string());
                                    }
                                    guppy::graph::GitReq::Tag(tag) => {
                                        dependency_details.tag = Some(tag.to_string());
                                    }
                                    guppy::graph::GitReq::Rev(rev) => {
                                        dependency_details.rev = Some(rev.to_string());
                                    }
                                    guppy::graph::GitReq::Default => {}
                                    _ => panic!("Unknown git requirements: {:?}", req),
                                }
                            }
                            _ => panic!("External source of unknown kind: {}", parsed_external),
                        }
                    } else {
                        panic!("Could not parse external source: {}", source);
                    }
                }
            }

            let dependency_name = if needs_rename {
                // TODO: this won't be unique if there are multiple versions of the same crate that have the same
                //   major/minor/patch version but differ in the pre-release version (e.g. `0.0.1-alpha` and `0.0.1-beta`).
                format!(
                    "{}_{}_{}_{}",
                    name, version.major, version.minor, version.patch
                )
            } else {
                name.to_string()
            };
            let dependency = Dependency::Detailed(dependency_details).simplify();

            dependencies.insert(dependency_name.clone(), dependency);
            package_ids2dependency_name
                .insert(package_id.to_owned(), dependency_name.replace("-", "_"));
        }
    }
    (dependencies, package_ids2dependency_name)
}

fn collect_package_ids<'a, I>(
    handler_pipelines: I,
    application_state_call_graph: &'a RawCallGraph,
    request_scoped_framework_bindings: &'a BiHashMap<Ident, ResolvedType>,
    codegen_deps: &'a HashMap<String, PackageId>,
    component_db: &'a ComponentDb,
    computation_db: &'a ComputationDb,
) -> IndexSet<PackageId>
where
    I: Iterator<Item = &'a RequestHandlerPipeline>,
{
    let mut package_ids = IndexSet::new();
    for t in request_scoped_framework_bindings.right_values() {
        collect_type_package_ids(&mut package_ids, t);
    }
    for package_id in codegen_deps.values() {
        package_ids.insert(package_id.to_owned());
    }
    collect_call_graph_package_ids(
        &mut package_ids,
        component_db,
        computation_db,
        application_state_call_graph,
    );
    for handler_pipeline in handler_pipelines {
        for graph in handler_pipeline.graph_iter() {
            collect_call_graph_package_ids(
                &mut package_ids,
                component_db,
                computation_db,
                &graph.call_graph,
            );
        }
    }
    package_ids
}

fn collect_call_graph_package_ids<'a>(
    package_ids: &mut IndexSet<PackageId>,
    component_db: &'a ComponentDb,
    computation_db: &'a ComputationDb,
    call_graph: &'a RawCallGraph,
) {
    for node in call_graph.node_weights() {
        match node {
            CallGraphNode::Compute { component_id, .. } => {
                let component = component_db.hydrated_component(*component_id, computation_db);
                match component.computation() {
                    Computation::Callable(c) => {
                        collect_callable_package_ids(package_ids, &c);
                    }
                    Computation::MatchResult(m) => {
                        collect_type_package_ids(package_ids, &m.input);
                        collect_type_package_ids(package_ids, &m.output);
                    }
                    Computation::PrebuiltType(i) => {
                        collect_type_package_ids(package_ids, &i);
                    }
                }
            }
            CallGraphNode::InputParameter { type_, .. } => {
                collect_type_package_ids(package_ids, type_)
            }
            CallGraphNode::MatchBranching => {}
        }
    }
}

fn collect_callable_package_ids(package_ids: &mut IndexSet<PackageId>, c: &Callable) {
    package_ids.insert(c.path.package_id.clone());
    for input in &c.inputs {
        collect_type_package_ids(package_ids, input);
    }
    if let Some(output) = c.output.as_ref() {
        collect_type_package_ids(package_ids, output);
    }
}

fn collect_type_package_ids(package_ids: &mut IndexSet<PackageId>, t: &ResolvedType) {
    match t {
        ResolvedType::ResolvedPath(t) => {
            package_ids.insert(t.package_id.clone());
            for generic in &t.generic_arguments {
                match generic {
                    GenericArgument::TypeParameter(t) => collect_type_package_ids(package_ids, t),
                    GenericArgument::Lifetime(_) => {}
                }
            }
        }
        ResolvedType::Reference(t) => collect_type_package_ids(package_ids, &t.inner),
        ResolvedType::Tuple(t) => {
            for element in &t.elements {
                collect_type_package_ids(package_ids, element)
            }
        }
        ResolvedType::Slice(s) => {
            collect_type_package_ids(package_ids, &s.element_type);
        }
        ResolvedType::Generic(_) | ResolvedType::ScalarPrimitive(_) => {}
    }
}
