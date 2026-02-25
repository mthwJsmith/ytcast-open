import http from 'node:http';
import { Innertube, UniversalCache } from 'youtubei.js';

const PORT = parseInt(process.env.PORT || '3033', 10);
const SECRET = process.env.YTRESOLVE_SECRET || '';

let yt;

async function init() {
  yt = await Innertube.create({
    cache: new UniversalCache(true, './cache'),
    generate_session_locally: true,
    retrieve_player: true,
  });
  console.log('[ytresolve] YouTube.js session ready');
}

async function resolve(videoId) {
  // Try getStreamingData (handles sig decryption automatically)
  const info = await yt.getBasicInfo(videoId);

  const title = info.basic_info?.title || videoId;
  const duration = info.basic_info?.duration || 0;

  // Pick best audio format with a direct URL
  const format = info.chooseFormat({ type: 'audio', quality: 'best' });
  const url = await format.decipher(yt.session.player);

  return { title, duration, url };
}

const server = http.createServer(async (req, res) => {
  // Auth check
  if (SECRET && req.headers['x-ytresolve-secret'] !== SECRET) {
    res.writeHead(403, { 'Content-Type': 'application/json' });
    res.end(JSON.stringify({ error: 'forbidden' }));
    return;
  }

  // Parse URL: /resolve/:videoId
  const match = req.url?.match(/^\/resolve\/([a-zA-Z0-9_-]{11})$/);
  if (!match) {
    res.writeHead(404, { 'Content-Type': 'application/json' });
    res.end(JSON.stringify({ error: 'not found', usage: '/resolve/:videoId' }));
    return;
  }

  const videoId = match[1];
  const start = Date.now();

  try {
    const result = await resolve(videoId);
    const ms = Date.now() - start;
    console.log(`[ytresolve] ${videoId} → ${result.title} (${ms}ms)`);
    res.writeHead(200, { 'Content-Type': 'application/json' });
    res.end(JSON.stringify(result));
  } catch (err) {
    const ms = Date.now() - start;
    console.error(`[ytresolve] ${videoId} FAILED (${ms}ms):`, err.message);
    res.writeHead(500, { 'Content-Type': 'application/json' });
    res.end(JSON.stringify({ error: err.message }));
  }
});

await init();
server.listen(PORT, () => {
  console.log(`[ytresolve] listening on :${PORT}`);
});
