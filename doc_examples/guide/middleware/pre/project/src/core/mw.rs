use pavex::http::{header::LOCATION, HeaderValue};
use pavex::middleware::Processing;
use pavex::request::RequestHead;
use pavex::response::Response;

/// If the request path ends with a `/`,
/// redirect to the same path without the trailing `/`.
pub fn redirect_to_normalized(request_head: &RequestHead) -> Processing {
    let Some(normalized_path) = request_head.target.path().strip_suffix('/') else {
        // No need to redirect, we continue processing the request.
        return Processing::Continue;
    };
    let location = HeaderValue::from_str(normalized_path).unwrap();
    let redirect = Response::temporary_redirect().insert_header(LOCATION, location);
    // Short-circuit the request processing pipeline and return a redirect response
    Processing::EarlyReturn(redirect)
}
