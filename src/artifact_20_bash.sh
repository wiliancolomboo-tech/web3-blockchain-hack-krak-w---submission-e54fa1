# Export production API credentials
export RESEND_API_KEY="re_your_production_key"
export SLACK_WEBHOOK_URL="https://api.example.com/webhooks/alerts/placeholder"

# 1-Click Launch via Docker Compose
docker compose up -d --build

# Verify live healthcheck
curl http://localhost:3000/health