import * as http from "http";

const HEALTH_PORT = process.env.PORT || 8080;
const server = http.createServer((req, res) => {
  if (req.url === "/healthz" && req.method === "GET") {
    res.writeHead(200, { "Content-Type": "application/json" });
    res.end(JSON.stringify({ status: "healthy", timestamp: new Date().toISOString() }));
  } else {
    res.writeHead(404);
    res.end();
  }
});
server.listen(HEALTH_PORT, () => {
  console.log(`[Telemetry] Healthcheck server live on :${HEALTH_PORT}/healthz`);
});