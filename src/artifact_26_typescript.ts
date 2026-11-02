import http from "http";

const healthServer = http.createServer((req, res) => {
  if (req.url === "/health") {
    res.writeHead(200, { "Content-Type": "application/json" });
    res.end(JSON.stringify({ status: "HEALTHY", uptime: process.uptime(), timestamp: Date.now() }));
    return;
  }
  res.writeHead(404);
  res.end();
});

healthServer.listen(8080, () => {
  console.log("[HealthProbe] HTTP healthcheck listening on port 8080");
});