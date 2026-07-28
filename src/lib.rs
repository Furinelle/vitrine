use worker::*;

/// Phase 0 scaffold: workers-rs entrypoint. Routes filled in Phase 1.
#[event(fetch)]
async fn main(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    console_error_panic_hook::set_once();

    let method = req.method();
    let url = req.url()?;
    let path = url.path().to_string();

    if method == Method::Options {
        return Ok(with_cors(Response::empty()?.with_status(204)));
    }

    if path == "/api/health" {
        return Ok(with_cors(Response::from_json(&serde_json::json!({
            "ok": true,
            "service": "shirogane"
        }))?));
    }

    if let Ok(assets) = env.assets("ASSETS") {
        return assets.fetch_request(req).await;
    }

    Response::ok("shirogane online")
}

fn with_cors(res: Response) -> Response {
    let headers = res.headers().clone();
    let _ = headers.set("Access-Control-Allow-Origin", "*");
    let _ = headers.set("Access-Control-Allow-Methods", "GET, POST, OPTIONS");
    let _ = headers.set(
        "Access-Control-Allow-Headers",
        "Authorization, Content-Type",
    );
    res.with_headers(headers)
}
