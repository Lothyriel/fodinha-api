import { readFile } from 'node:fs/promises';

const grafanaUrl = process.env.GRAFANA_URL ?? 'https://fly-metrics.net';
const grafanaToken = process.env.GRAFANA_TOKEN;
const folderUid = process.env.GRAFANA_FOLDER_UID ?? null;
const dashboardUid = process.env.GRAFANA_DASHBOARD_UID ?? 'oh-hell-observability';
const dashboardTitle = process.env.GRAFANA_DASHBOARD_TITLE ?? 'Oh Hell Observability';
const commitMessage = process.env.GRAFANA_COMMIT_MESSAGE ?? 'sync dashboard from repo';

if (!grafanaToken) {
  console.error('Missing GRAFANA_TOKEN');
  process.exit(1);
}

const file = new URL('../monitoring/fly-grafana-dashboard.json', import.meta.url);
const dashboard = JSON.parse(await readFile(file, 'utf8'));

dashboard.uid = dashboardUid;
dashboard.title = dashboardTitle;

const response = await fetch(`${grafanaUrl}/api/dashboards/db`, {
  method: 'POST',
  headers: {
    Authorization: `Bearer ${grafanaToken}`,
    'Content-Type': 'application/json',
    Accept: 'application/json',
  },
  body: JSON.stringify({
    dashboard,
    folderUid,
    message: commitMessage,
    overwrite: true,
  }),
});

if (!response.ok) {
  console.error(`Grafana import failed: ${response.status} ${response.statusText}`);
  console.error(await response.text());
  process.exit(1);
}

const result = await response.json();
console.log(JSON.stringify(result, null, 2));
