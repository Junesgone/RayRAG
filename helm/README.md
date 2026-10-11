# RayRAG Helm chart

Minimal chart covering the deployment semantics of upstream RAGFlow's
`helm/templates` (14 K8s templates): a single Deployment + Service + ConfigMap
for the RayRAG image `rayrag:local`. The Service exposes port **9390** and
targets the container port **8080** (the image default `serve --port 8080`).

## Install

```bash
docker build -t rayrag:local .          # or: docker compose build rayrag
helm install rayrag ./helm \
  --set config.rayragPostgresUrl="postgresql://admin:admin@postgres:5432/rayrag"
```

## PostgreSQL dependency (required)

Upstream RAGFlow's Helm chart bundles Elasticsearch/MinIO/MySQL/OpenSearch/
Redis/Infinity sub-resources. RayRAG intentionally replaces all of them with a
single **external PostgreSQL 18** dependency — vector search runs in-process
(zvec backend), and the remaining services are not part of the Rust runtime.

Bring your own PostgreSQL:

```bash
helm repo add bitnami https://charts.bitnami.com/bitnami
helm install postgres bitnami/postgresql \
  --set auth.database=rayrag --set auth.username=admin \
  --set auth.password=admin --set image.tag=18.4
```

then point `config.rayragPostgresUrl` at it. For production, move the password
into a Secret (the Deployment already reads `RAYRAG_ADMIN_PASSWORD` from a
Secret named `<release>-rayrag`, key `rayrag-admin-password`, when present).

## Verify

```bash
helm lint ./helm
helm template rayrag ./helm | kubectl apply --dry-run=client -f -
```

Required env (mirroring docker-compose): `RAYRAG_POSTGRES_URL`,
`RAYRAG_ADMIN_PASSWORD`. All other env vars have safe defaults in the
ConfigMap/values.yaml.
