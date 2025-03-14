use std::collections::BTreeSet;

use guppy::PackageId;

use super::auxiliary::AuxiliaryData;
use crate::{diagnostic::DiagnosticSink, rustdoc::CrateCollection};

/// The identifier of the interned annotation metadata.
pub type AnnotationId = la_arena::Idx<AnnotationIdentifiers>;

/// Information required to retrieve the annotated item from JSON documentation.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AnnotationIdentifiers {
    /// The package ID of the crate that defined the annotated item.
    package_id: guppy::PackageId,
    /// The ID of the item within the crate associated with
    /// [`Self::package_id`].
    item_id: rustdoc_types::Id,
}

/// Return the set of package IDs that may contain annotated components
/// that are in scope for the blueprint we are processing.
pub fn annotation_sources<'a>(
    server_sdk_id: &PackageId,
    krate_collection: &'a CrateCollection,
) -> BTreeSet<&'a PackageId> {
    let mut cache = krate_collection.package_graph().new_depends_cache();
    krate_collection
        .package_graph()
        .packages()
        .filter(|p| {
            use guppy::graph::DependencyDirection;

            // To contain annotated components, a package must depend on the `pavex` crate,
            // since that's where the annotation macros are defined.
            let depends_on_pavex = p
                .direct_links_directed(DependencyDirection::Forward)
                .any(|id| id.to().name() == "pavex");
            let mut in_scope = depends_on_pavex;
            if p.in_workspace() {
                // If it's a workspace member, it must not depend on the server SDK (or be the server SDK itself)
                in_scope = in_scope
                    && p.id() != server_sdk_id
                    && !cache.depends_on(p.id(), server_sdk_id).ok().unwrap_or(true);
            }
            in_scope
        })
        .map(|p| p.id())
        .collect()
}

/// Process all annotated components.
pub(super) fn process_annotations(
    aux: &mut AuxiliaryData,
    server_sdk_id: &PackageId,
    krate_collection: &CrateCollection,
    diagnostics: &mut DiagnosticSink,
) {
    // Process annotations here
}
