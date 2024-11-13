use pavex::blueprint::router::GET;
use pavex::blueprint::Blueprint;
use pavex::f;

pub fn bp() -> Blueprint {
    let mut bp = Blueprint::new();
    // Serve the website from the root domain...
    bp.domain("pavex.dev").nest(website_bp());
    // ...while reserving a subdomain for the REST API.
    bp.domain("api.pavex.dev").prefix("/v1").nest(api_bp());
    bp
}

fn website_bp() -> Blueprint {
    let mut bp = Blueprint::new();
    bp.route(GET, "/", f!(super::index));
    // Other web pages...
    bp
}

fn api_bp() -> Blueprint {
    let mut bp = Blueprint::new();
    bp.route(GET, "/users", f!(super::users));
    // Other API routes...
    bp
}
