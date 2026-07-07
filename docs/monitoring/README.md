# Monitoring

Fly.io managed Grafana scrapes Prometheus-format metrics from `/metrics`.

## App setup

This repo configures Fly scraping in `fly.toml`:

```toml
[metrics]
  port = 3000
  path = "/metrics"
```

The API exposes Prometheus metrics at `GET /metrics`.

## Dashboard as code

Dashboard definition lives in:

- `monitoring/fly-grafana-dashboard.json`

Import or update it with:

```bash
GRAFANA_URL="https://fly-metrics.net" \
GRAFANA_TOKEN="<grafana-api-token>" \
node ./scripts/import-grafana-dashboard.mjs
```

Optional env:

- `GRAFANA_FOLDER_UID`
- `GRAFANA_DASHBOARD_UID`
- `GRAFANA_DASHBOARD_TITLE`
- `GRAFANA_COMMIT_MESSAGE`

## Notes

- Fly managed Grafana uses your organization's Prometheus datasource automatically.
- The dashboard uses a Prometheus datasource variable, so it can be imported into other Grafana instances too.
- You still need a Grafana API token for automated import. If Fly's managed Grafana does not allow token creation in your org, the JSON file can still be imported manually.
