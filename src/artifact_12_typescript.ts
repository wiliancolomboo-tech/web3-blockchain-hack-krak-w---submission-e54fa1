import * as http from "http";

const HEALTH_PORT = process.env.PORT || 8080;

const healthServer = http.createServer((req, res) => {
  if (req.url === "/health" && req.method === "GET") {
    res.writeHead(200, { "Content-Type": "application/json" });
    res.end(
      JSON.stringify({
        status: "HEALTHY",
        protocol: "KrakFlow Engine",
        uptime: process.uptime(),
        timestamp: new Date().toISOString(),
        rpcConnected: connection !== undefined,
      })
    );
  } else {
    res.writeHead(404);
    res.end();
  }
});

healthServer.listen(HEALTH_PORT, () => {
  console.log(`[HEALTH] Turnkey operational healthcheck listening on port :${HEALTH_PORT}/health`);
});