pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Debug, Clone)]
pub struct StreamInfo {
    pub title: String,
    pub duration: u64,
    pub stream_url: String,
}

/// Resolve a video ID to a playable audio stream URL.
///
/// Makes a lightweight InnerTube /player call to get the real title and
/// duration (needed for YouTube seekbar sync), then returns a localhost URL
/// that the DIAL server will handle via SABR when MPD connects.
pub async fn resolve_stream(
    client: &reqwest::Client,
    video_id: &str,
    ctt: Option<&str>,
    _playlist_id: Option<&str>,
) -> Result<Option<StreamInfo>> {
    let stream_url = format!("http://localhost:8008/stream/{}", video_id);

    // When ctt is available, use WEB_REMIX (YouTube Music web client) which
    // is the only client that accepts credentialTransferTokens from cast sessions.
    let (title, duration) = if let Some(token) = ctt {
        match web_remix_player(client, video_id, token).await {
            Ok(td) => td,
            Err(e) => {
                tracing::warn!("[innertube] WEB_REMIX+ctt failed: {}, trying ANDROID", e);
                android_player(client, video_id).await.unwrap_or_else(|e2| {
                    tracing::warn!("[innertube] ANDROID failed: {}, using fallback", e2);
                    (video_id.to_owned(), 0)
                })
            }
        }
    } else {
        android_player(client, video_id).await.unwrap_or_else(|e| {
            tracing::warn!("[innertube] ANDROID failed: {}, using fallback", e);
            (video_id.to_owned(), 0)
        })
    };

    tracing::info!("[stream] {video_id} → \"{}\" [{}s] via SABR", title, duration);
    Ok(Some(StreamInfo {
        title,
        duration,
        stream_url,
    }))
}

async fn web_remix_player(
    client: &reqwest::Client,
    video_id: &str,
    ctt: &str,
) -> Result<(String, u64)> {
    let body = serde_json::json!({
        "videoId": video_id,
        "context": {
            "client": {
                "clientName": "WEB_REMIX",
                "clientVersion": "1.20241120.01.00",
                "hl": "en",
                "gl": "US",
            },
            "user": {
                "credentialTransferTokens": [{
                    "scope": "VIDEO",
                    "token": ctt,
                }]
            }
        },
        "contentCheckOk": true,
        "racyCheckOk": true,
    });

    let resp = client
        .post("https://music.youtube.com/youtubei/v1/player?prettyPrint=false")
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36")
        .header("Content-Type", "application/json")
        .header("Origin", "https://music.youtube.com")
        .header("Referer", "https://music.youtube.com/")
        .json(&body)
        .send()
        .await?;

    if !resp.status().is_success() {
        return Err(format!("/player HTTP {}", resp.status()).into());
    }

    parse_player_response(video_id, resp).await
}

async fn android_player(
    client: &reqwest::Client,
    video_id: &str,
) -> Result<(String, u64)> {
    let body = serde_json::json!({
        "videoId": video_id,
        "context": {
            "client": {
                "clientName": "ANDROID",
                "clientVersion": "19.44.38",
                "androidSdkVersion": 34,
                "hl": "en",
                "gl": "US",
            }
        },
        "contentCheckOk": true,
        "racyCheckOk": true,
    });

    let resp = client
        .post("https://www.youtube.com/youtubei/v1/player?prettyPrint=false")
        .header("User-Agent", "com.google.android.youtube/19.44.38 (Linux; U; Android 14; en_US; Pixel 8) gzip")
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await?;

    if !resp.status().is_success() {
        return Err(format!("/player HTTP {}", resp.status()).into());
    }

    parse_player_response(video_id, resp).await
}

async fn parse_player_response(
    video_id: &str,
    resp: reqwest::Response,
) -> Result<(String, u64)> {
    let json: serde_json::Value = resp.json().await?;
    let status = json["playabilityStatus"]["status"]
        .as_str()
        .unwrap_or("UNKNOWN");
    if status != "OK" {
        let reason = json["playabilityStatus"]["reason"]
            .as_str()
            .unwrap_or("unknown");
        tracing::warn!("[innertube] {} not playable: {} ({})", video_id, status, reason);
        return Err(format!("{}: {} ({})", video_id, status, reason).into());
    }
    let title = json["videoDetails"]["title"]
        .as_str()
        .unwrap_or(video_id)
        .to_owned();
    let duration = json["videoDetails"]["lengthSeconds"]
        .as_str()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);
    Ok((title, duration))
}
