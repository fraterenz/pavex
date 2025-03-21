use pavex::blueprint::{router::GET, Blueprint};
use pavex::f;

pub fn blueprint() -> Blueprint {
    let mut bp = Blueprint::new();
    bp.route(
        GET,
        "/greet/{first_name}/{last_name}",
        f!(crate::routes::greet),
    );
    bp
}
