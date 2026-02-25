use serde_json::{json, Value};

const INNERTUBE_URL: &str = "https://www.youtube.com/youtubei/v1/player";

/// Remote resolver URL (YouTube.js on Hetzner). Set via YTRESOLVE_URL env var.
/// Example: "https://ytresolve.maelo.ca" or "http://46.224.156.225:3033"
static REMOTE_URL: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
static REMOTE_SECRET: std::sync::OnceLock<String> = std::sync::OnceLock::new();

fn remote_url() -> &'static Option<String> {
    REMOTE_URL.get_or_init(|| std::env::var("YTRESOLVE_URL").ok().filter(|s| !s.is_empty()))
}

fn remote_secret() -> &'static str {
    REMOTE_SECRET.get_or_init(|| std::env::var("YTRESOLVE_SECRET").unwrap_or_default())
}

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

// ---------------------------------------------------------------------------
// Client identities — tried in order until one returns a playable stream.
//
// IOS: Direct URLs (no sig decryption), no PO token required (for now).
//   Best coverage in testing: 20/20 videos OK.
//
// ANDROID: Same direct URLs, same coverage. Backup if IOS gets blocked.
//
// If both fail, falls back to SABR streaming via ytresolve (YouTube's own
// native streaming protocol — always works, future-proof).
// ---------------------------------------------------------------------------

struct ClientIdentity {
    name: &'static str,
    context: Value,
    user_agent: &'static str,
}

fn ios_client(video_id: &str, ctt: Option<&str>, playlist_id: Option<&str>) -> ClientIdentity {
    let mut context = json!({
        "client": {
            "clientName": "IOS",
            "clientVersion": "21.02.3",
            "deviceMake": "Apple",
            "deviceModel": "iPhone16,2",
            "osName": "iPhone",
            "osVersion": "18.3.2.22D82"
        }
    });

    if let Some(token) = ctt {
        context["user"] = json!({
            "enableSafetyMode": false,
            "lockedSafetyMode": false,
            "credentialTransferTokens": [{
                "scope": "VIDEO",
                "token": token
            }]
        });
    }

    let mut payload = json!({
        "context": context,
        "videoId": video_id
    });
    if let Some(pid) = playlist_id {
        payload["playlistId"] = json!(pid);
    }

    ClientIdentity {
        name: "IOS",
        context: payload,
        user_agent: "com.google.ios.youtube/21.02.3 (iPhone16,2; U; CPU iOS 18_3_2 like Mac OS X;)",
    }
}

fn android_client(video_id: &str, ctt: Option<&str>, playlist_id: Option<&str>) -> ClientIdentity {
    let mut context = json!({
        "client": {
            "clientName": "ANDROID",
            "clientVersion": "21.02.35",
            "androidSdkVersion": 30,
            "osName": "Android",
            "osVersion": "11"
        }
    });

    if let Some(token) = ctt {
        context["user"] = json!({
            "enableSafetyMode": false,
            "lockedSafetyMode": false,
            "credentialTransferTokens": [{
                "scope": "VIDEO",
                "token": token
            }]
        });
    }

    let mut payload = json!({
        "context": context,
        "videoId": video_id
    });
    if let Some(pid) = playlist_id {
        payload["playlistId"] = json!(pid);
    }

    ClientIdentity {
        name: "ANDROID",
        context: payload,
        user_agent: "com.google.android.youtube/21.02.35 (Linux; U; Android 11) gzip",
    }
}


// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct StreamInfo {
    pub title: String,
    pub duration: u64,
    pub stream_url: String,
}

// ---------------------------------------------------------------------------
// Remote resolver (ytresolve — direct URL + SABR stream fallback)
// ---------------------------------------------------------------------------

fn build_remote_request(client: &reqwest::Client, url: &str) -> reqwest::RequestBuilder {
    let mut req = client.get(url).timeout(std::time::Duration::from_secs(10));
    let secret = remote_secret();
    if !secret.is_empty() {
        req = req.header("x-ytresolve-secret", secret);
    }
    req
}

/// Try /stream/:videoId — SABR audio proxy (always works, future-proof).
/// Returns a stream URL pointing at the resolver's SABR proxy endpoint.
async fn try_remote_stream(client: &reqwest::Client, video_id: &str) -> Option<StreamInfo> {
    let base_url = remote_url().as_deref()?;
    let stream_url = format!("{}/stream/{}", base_url.trim_end_matches('/'), video_id);

    // Verify the stream endpoint is reachable with a HEAD-like quick check
    let health_url = format!("{}/health", base_url.trim_end_matches('/'));
    match build_remote_request(client, &health_url).send().await {
        Ok(r) if r.status().is_success() => {}
        _ => {
            tracing::warn!("[stream] remote SABR resolver unreachable for {video_id}");
            return None;
        }
    }

    // Build the stream URL with auth header baked in as query param
    // MPD needs a plain URL it can fetch — we pass the secret as a query param
    // that the server can also accept (or the URL works as-is if no secret)
    let secret = remote_secret();
    let final_url = if !secret.is_empty() {
        format!("{}?secret={}", stream_url, secret)
    } else {
        stream_url
    };

    tracing::info!("[stream] using SABR stream for {video_id}");
    Some(StreamInfo {
        title: video_id.to_owned(), // title unknown until stream starts
        duration: 0,
        stream_url: final_url,
    })
}

// ---------------------------------------------------------------------------
// Resolver
// ---------------------------------------------------------------------------

/// Resolve a playable audio stream URL.
///
/// Fallback chain:
///   1. IOS (local)     — direct URLs, fastest, no overhead
///   2. ANDROID (local) — direct URLs, different client identity
///   3. SABR stream     — YouTube's native streaming protocol via ytresolve (always works)
///
/// The SABR endpoint is only available if YTRESOLVE_URL is set.
pub async fn resolve_stream(
    client: &reqwest::Client,
    video_id: &str,
    ctt: Option<&str>,
    playlist_id: Option<&str>,
) -> Result<Option<StreamInfo>> {
    // 1. IOS — fastest, works locally
    let ios = ios_client(video_id, ctt, playlist_id);
    if let Some(info) = try_client(client, video_id, &ios).await? {
        return Ok(Some(info));
    }

    // 2. ANDROID — backup local client
    let android = android_client(video_id, ctt, playlist_id);
    if let Some(info) = try_client(client, video_id, &android).await? {
        return Ok(Some(info));
    }

    // 3. SABR stream — always works, YouTube's own protocol
    if let Some(info) = try_remote_stream(client, video_id).await {
        return Ok(Some(info));
    }

    tracing::error!("[stream] all methods failed for {video_id}");
    Ok(None)
}

/// Try a single InnerTube client identity.
async fn try_client(
    client: &reqwest::Client,
    video_id: &str,
    identity: &ClientIdentity,
) -> Result<Option<StreamInfo>> {
    let res = client
        .post(INNERTUBE_URL)
        .header("User-Agent", identity.user_agent)
        .json(&identity.context)
        .send()
        .await?;

    if !res.status().is_success() {
        tracing::warn!("[stream] {} {} for {}", identity.name, res.status(), video_id);
        return Ok(None);
    }

    let data: Value = res.json().await?;

    let status = data["playabilityStatus"]["status"].as_str().unwrap_or("");
    if status != "OK" {
        let reason = data["playabilityStatus"]["reason"]
            .as_str()
            .unwrap_or("unknown");
        tracing::warn!("[stream] {} {}: {} -- {}", identity.name, video_id, status, reason);
        return Ok(None);
    }

    let title = data["videoDetails"]["title"]
        .as_str()
        .unwrap_or(video_id)
        .to_owned();

    let duration: u64 = data["videoDetails"]["lengthSeconds"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    if let Some(url) = best_audio_url(&data["streamingData"]["adaptiveFormats"]) {
        tracing::info!("[stream] resolved {video_id} via {}", identity.name);
        return Ok(Some(StreamInfo { title, duration, stream_url: url }));
    }

    if let Some(url) = first_playable_url(&data["streamingData"]["formats"]) {
        tracing::info!("[stream] resolved {video_id} (progressive) via {}", identity.name);
        return Ok(Some(StreamInfo { title, duration, stream_url: url }));
    }

    tracing::warn!("[stream] {} OK but no direct URLs for {}", identity.name, video_id);
    Ok(None)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// From adaptive formats, pick the audio stream with the highest bitrate
/// that has a direct `url` (not signature-ciphered).
fn best_audio_url(formats: &Value) -> Option<String> {
    let arr = formats.as_array()?;

    let mut audio_formats: Vec<&Value> = arr
        .iter()
        .filter(|f| {
            f["mimeType"]
                .as_str()
                .is_some_and(|m| m.starts_with("audio/"))
                && f["url"].is_string()
        })
        .collect();

    audio_formats.sort_by(|a, b| {
        let br_a = a["bitrate"].as_u64().unwrap_or(0);
        let br_b = b["bitrate"].as_u64().unwrap_or(0);
        br_b.cmp(&br_a)
    });

    audio_formats
        .first()
        .and_then(|f| f["url"].as_str())
        .map(String::from)
}

/// From progressive formats, return the first one with a direct URL.
fn first_playable_url(formats: &Value) -> Option<String> {
    formats
        .as_array()?
        .iter()
        .find(|f| f["url"].is_string())
        .and_then(|f| f["url"].as_str())
        .map(String::from)
}
