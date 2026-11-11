use std::io::Cursor;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use dn_media::{MediaConfig, MediaEvent, MediaLoader};
use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn png(w: u32, h: u32) -> Vec<u8> {
    let img = DynamicImage::ImageRgba8(RgbaImage::from_pixel(w, h, Rgba([30, 60, 90, 255])));
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, ImageFormat::Png).unwrap();
    out.into_inner()
}

fn loader(cfg: MediaConfig) -> (MediaLoader, Arc<AtomicUsize>) {
    let wakes = Arc::new(AtomicUsize::new(0));
    let counter = wakes.clone();
    let on_ready = Arc::new(move || {
        counter.fetch_add(1, Ordering::SeqCst);
    });
    (MediaLoader::new(cfg, tokio::runtime::Handle::current(), on_ready), wakes)
}

fn cached_in(dir: &Path) -> MediaConfig {
    MediaConfig { disk_dir: Some(dir.to_path_buf()), ..MediaConfig::default() }
}

async fn expect_events(loader: &MediaLoader, count: usize) -> Vec<MediaEvent> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut events = Vec::new();
    while events.len() < count && Instant::now() < deadline {
        events.extend(loader.drain());
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(events.len(), count, "unexpected events: {events:?}");
    events
}

async fn hits(server: &MockServer) -> usize {
    server.received_requests().await.unwrap().len()
}

fn ready(event: MediaEvent) -> dn_media::DecodedImage {
    match event {
        MediaEvent::Ready(img) => img,
        MediaEvent::Failed { url, error } => panic!("{url} failed: {error}"),
    }
}

fn failed(event: MediaEvent) -> String {
    match event {
        MediaEvent::Failed { error, .. } => error,
        MediaEvent::Ready(img) => panic!("{} unexpectedly decoded", img.url),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fetches_decodes_and_downscales() {
    let server = MockServer::start().await;
    Mock::given(method("GET")).and(path("/big.png")).respond_with(ResponseTemplate::new(200).set_body_bytes(png(400, 200))).mount(&server).await;

    let (loader, wakes) = loader(MediaConfig::default());
    let url = format!("{}/big.png", server.uri());
    loader.request(&url, 100);

    let img = ready(expect_events(&loader, 1).await.remove(0));
    assert_eq!(img.url, url);
    assert_eq!((img.width, img.height), (100, 50));
    assert_eq!(img.rgba.len(), 100 * 50 * 4);
    assert_eq!(&img.rgba[..4], &[30, 60, 90, 255]);
    assert!(wakes.load(Ordering::SeqCst) >= 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sends_the_application_user_agent() {
    let server = MockServer::start().await;
    Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_bytes(png(4, 4))).mount(&server).await;

    let (loader, _) = loader(MediaConfig::default());
    loader.request(&format!("{}/a.png", server.uri()), 64);
    expect_events(&loader, 1).await;

    let requests = server.received_requests().await.unwrap();
    let ua = requests[0].headers.get("user-agent").unwrap().to_str().unwrap().to_owned();
    assert_eq!(ua, dn_core::user_agent());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_and_repeated_requests_hit_the_network_once() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/a.png"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(png(8, 8)).set_delay(Duration::from_millis(150)))
        .mount(&server)
        .await;

    let (loader, _) = loader(MediaConfig::default());
    let url = format!("{}/a.png", server.uri());
    loader.request(&url, 64);
    loader.request(&url, 64);
    loader.clone().request(&url, 32);
    expect_events(&loader, 1).await;

    // Delivered and not forgotten: further requests are no-ops.
    loader.request(&url, 64);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(loader.drain().is_empty());
    assert_eq!(hits(&server).await, 1);

    // After the UI evicts the texture the image can be loaded again.
    loader.forget(&url);
    loader.request(&url, 64);
    expect_events(&loader, 1).await;
    assert_eq!(hits(&server).await, 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disk_cache_serves_a_second_loader_without_network() {
    let server = MockServer::start().await;
    Mock::given(method("GET")).and(path("/img.png")).respond_with(ResponseTemplate::new(200).set_body_bytes(png(64, 32))).mount(&server).await;
    let dir = tempfile::tempdir().unwrap();

    let (first, _) = loader(cached_in(dir.path()));
    first.request(&format!("{}/img.png?ex=1&is=2&hm=3&width=64", server.uri()), 64);
    ready(expect_events(&first, 1).await.remove(0));
    assert_eq!(hits(&server).await, 1);
    assert!(first.disk_usage() > 0);

    // New instance, refreshed signature, different max_dim: still a cache hit.
    let (second, _) = loader(cached_in(dir.path()));
    second.request(&format!("{}/img.png?ex=9&is=8&hm=7&width=64", server.uri()), 16);
    let img = ready(expect_events(&second, 1).await.remove(0));
    assert_eq!((img.width, img.height), (16, 8));
    assert_eq!(hits(&server).await, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn startup_prune_enforces_the_disk_budget() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..4 {
        std::fs::write(dir.path().join(format!("f{i}")), vec![0u8; 1000]).unwrap();
    }
    let cfg = MediaConfig { disk_budget_bytes: 2500, ..cached_in(dir.path()) };
    let (loader, _) = loader(cfg);

    let deadline = Instant::now() + Duration::from_secs(10);
    while loader.disk_usage() == 0 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(loader.disk_usage() <= 2500);
    assert!(std::fs::read_dir(dir.path()).unwrap().count() < 4);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http_errors_fail_then_honour_the_cooldown() {
    let server = MockServer::start().await;
    Mock::given(method("GET")).and(path("/missing.png")).respond_with(ResponseTemplate::new(404)).mount(&server).await;

    let cfg = MediaConfig { failure_cooldown: Duration::from_millis(400), ..MediaConfig::default() };
    let (loader, _) = loader(cfg);
    let url = format!("{}/missing.png", server.uri());

    loader.request(&url, 64);
    let error = failed(expect_events(&loader, 1).await.remove(0));
    assert!(error.contains("404"), "{error}");

    loader.request(&url, 64);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(loader.drain().is_empty());
    assert_eq!(hits(&server).await, 1);

    tokio::time::sleep(Duration::from_millis(400)).await;
    loader.request(&url, 64);
    failed(expect_events(&loader, 1).await.remove(0));
    assert_eq!(hits(&server).await, 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn corrupt_bytes_fail_and_are_not_cached() {
    let server = MockServer::start().await;
    Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_bytes(b"<html>not an image</html>".to_vec())).mount(&server).await;
    let dir = tempfile::tempdir().unwrap();

    let (loader, _) = loader(cached_in(dir.path()));
    loader.request(&format!("{}/x.png", server.uri()), 64);
    let error = failed(expect_events(&loader, 1).await.remove(0));
    assert!(error.contains("decode"), "{error}");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unreadable_cached_file_is_replaced_from_the_network() {
    let server = MockServer::start().await;
    Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_bytes(png(8, 8))).mount(&server).await;
    let dir = tempfile::tempdir().unwrap();

    let (first, _) = loader(cached_in(dir.path()));
    first.request(&format!("{}/x.png", server.uri()), 64);
    expect_events(&first, 1).await;
    for entry in std::fs::read_dir(dir.path()).unwrap().flatten() {
        std::fs::write(entry.path(), b"garbage").unwrap();
    }

    let (second, _) = loader(cached_in(dir.path()));
    second.request(&format!("{}/x.png", server.uri()), 64);
    ready(expect_events(&second, 1).await.remove(0));
    assert_eq!(hits(&server).await, 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn downloads_over_the_size_cap_fail() {
    let server = MockServer::start().await;
    let body = png(256, 256);
    Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_bytes(body.clone())).mount(&server).await;

    let cfg = MediaConfig { max_download_bytes: (body.len() / 2) as u64, ..MediaConfig::default() };
    let (loader, _) = loader(cfg);
    loader.request(&format!("{}/big.png", server.uri()), 64);
    let error = failed(expect_events(&loader, 1).await.remove(0));
    assert!(error.contains("exceeds"), "{error}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn non_http_schemes_and_garbage_urls_fail_without_network() {
    let (loader, _) = loader(MediaConfig::default());
    loader.request("file:///etc/passwd", 64);
    loader.request("ftp://example.com/a.png", 64);
    loader.request("not a url", 64);
    loader.request("", 64);
    let events = expect_events(&loader, 3).await;
    assert!(events.into_iter().all(|e| matches!(e, MediaEvent::Failed { .. })));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_all_drops_in_flight_work_and_resets_state() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(png(8, 8)).set_delay(Duration::from_millis(300)))
        .mount(&server)
        .await;

    let (loader, _) = loader(MediaConfig::default());
    let url = format!("{}/slow.png", server.uri());
    loader.request(&url, 64);
    tokio::time::sleep(Duration::from_millis(50)).await;
    loader.cancel_all();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(loader.drain().is_empty());

    loader.request(&url, 64);
    ready(expect_events(&loader, 1).await.remove(0));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn many_urls_complete_with_limited_concurrency() {
    let server = MockServer::start().await;
    Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_bytes(png(16, 16)).set_delay(Duration::from_millis(20))).mount(&server).await;

    let cfg = MediaConfig { max_concurrent: 2, ..MediaConfig::default() };
    let (loader, _) = loader(cfg);
    for i in 0..12 {
        loader.request(&format!("{}/{i}.png", server.uri()), 64);
    }
    let events = expect_events(&loader, 12).await;
    assert!(events.into_iter().all(|e| matches!(e, MediaEvent::Ready(_))));
}
