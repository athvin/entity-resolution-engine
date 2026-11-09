//! Real TCP requests through the attempt router and a rotating HTTP JWKS
//! source. Production timing is unchanged; the minute is deliberately real.
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

use axum::{Router, extract::State, routing::get};
use tokio::sync::RwLock;

use super::{
    AppState, BoxFuture, JwksSource, StatusCode, TEST_JWKS, TEST_PEM, Verifier, app, mint_with_key,
};

const ROTATED_PEM: &str = include_str!("../../testdata/sa_token_rotated_test_rsa.pem");
const ROTATED_JWKS: &str = include_str!("../../testdata/sa_token_rotated_test_jwks.json");

#[derive(Clone)]
struct Keys {
    body: Arc<RwLock<String>>,
    fetches: Arc<AtomicUsize>,
}

async fn jwks(State(keys): State<Keys>) -> String {
    keys.fetches.fetch_add(1, Ordering::SeqCst);
    keys.body.read().await.clone()
}

struct HttpSource {
    client: reqwest::Client,
    url: String,
}

impl JwksSource for HttpSource {
    fn fetch(&self) -> BoxFuture<'_, Result<String, String>> {
        Box::pin(async move {
            self.client
                .get(&self.url)
                .send()
                .await
                .map_err(|e| e.to_string())?
                .error_for_status()
                .map_err(|e| e.to_string())?
                .text()
                .await
                .map_err(|e| e.to_string())
        })
    }
}

struct Server(tokio::task::JoinHandle<()>);

impl Drop for Server {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn serve(router: Router) -> (String, Server) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (url, Server(task))
}

async fn status(client: &reqwest::Client, url: &str, token: &str) -> StatusCode {
    client
        .get(url)
        .bearer_auth(token)
        .send()
        .await
        .unwrap()
        .status()
}

pub(super) async fn exercise(dsn: &str, attempt: &str, job_name: &str) {
    let keys = Keys {
        body: Arc::new(RwLock::new(TEST_JWKS.to_string())),
        fetches: Arc::new(AtomicUsize::new(0)),
    };
    let (jwks_url, _jwks_server) = serve(
        Router::new()
            .route("/openid/v1/jwks", get(jwks))
            .with_state(keys.clone()),
    )
    .await;
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    let verifier = Verifier::new(Box::new(HttpSource {
        client: client.clone(),
        url: format!("{jwks_url}/openid/v1/jwks"),
    }));
    let (api_url, _api_server) = serve(app(AppState::with_pg(
        "op-secret".to_string(),
        dsn.to_string(),
    )
    .with_sa_verifier(Some(verifier))))
    .await;
    let url = format!("{api_url}/v1/internal/attempts/{attempt}");
    let pod = format!("{job_name}-x7b2k");
    let old = mint_with_key(&pod, "uid-from-claims", "test-kid-1", TEST_PEM);
    let new = mint_with_key(&pod, "uid-from-claims", "test-kid-2", ROTATED_PEM);
    assert_eq!(status(&client, &url, &old).await, StatusCode::OK);
    assert_eq!(keys.fetches.load(Ordering::SeqCst), 1);

    *keys.body.write().await = ROTATED_JWKS.to_string();
    let mut burst = tokio::task::JoinSet::new();
    for _ in 0..16 {
        let (client, url, token) = (client.clone(), url.clone(), new.clone());
        burst.spawn(async move { status(&client, &url, &token).await });
    }
    while let Some(result) = burst.join_next().await {
        assert_eq!(result.unwrap(), StatusCode::UNAUTHORIZED);
    }
    assert_eq!(
        keys.fetches.load(Ordering::SeqCst),
        1,
        "unknown-key burst inside the floor must not fetch"
    );
    assert_eq!(
        status(&client, &url, &old).await,
        StatusCode::OK,
        "cached key remains valid until refresh"
    );

    tokio::time::sleep(Duration::from_secs(61)).await;
    assert_eq!(
        status(&client, &url, &new).await,
        StatusCode::OK,
        "new signing key is accepted after a real floor interval"
    );
    assert_eq!(keys.fetches.load(Ordering::SeqCst), 2);
    for _ in 0..8 {
        assert_eq!(
            status(&client, &url, &old).await,
            StatusCode::UNAUTHORIZED,
            "removed key is rejected even though its token has not expired"
        );
    }
    let forged = mint_with_key(&pod, "uid-from-claims", "test-kid-2", TEST_PEM);
    assert_eq!(
        status(&client, &url, &forged).await,
        StatusCode::UNAUTHORIZED,
        "a new kid with the old signature cannot verify"
    );
    assert_eq!(status(&client, &url, &new).await, StatusCode::OK);
    assert_eq!(
        keys.fetches.load(Ordering::SeqCst),
        2,
        "retired keys cannot cause a fetch storm"
    );
}
