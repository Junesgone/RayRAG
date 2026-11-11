# RayRAG for 自托管主机 (FPK package)

RayRAG ships as a native 自托管主机 application: install the `.fpk` from the 自托管主机 App Center,
open the desktop icon, and the Rust server starts with its own bundled PostgreSQL 18 and
zvec vector store. No Docker, no external database, no MySQL.

- Package ID: `rayrag`
- Default port: **9395** (open `http://<NAS-IP>:9395`)
- First login: **register any account**; the first registration becomes the administrator (the first-run guide then writes the configuration).
- Data directory (visible in File Manager): `rayrag/` share on the volume you pick

---

## Install on 自托管主机

1. Download `rayrag_<version>_x86_64.fpk` (GitHub Releases of this repository).
2. Open the 自托管主机 **App Center**, click **Manual install** (设置 → 应用中心 → 手动安装),
   choose the `.fpk` file and confirm.
3. The install wizard asks for the bundled PostgreSQL password. Keep `admin`
   (the default) or choose your own (letters, digits and `. _ -`). The same value is
   written to the RayRAG environment file.
4. Wait for the install to finish; the app card shows **Start**. RayRAG starts
   automatically after install.
5. Click the RayRAG card (or open `http://<NAS-IP>:9395`) and sign in with
register any account; the first registration becomes the administrator.

The first-run guide collects the model provider, embedding model and deployment options.
Those settings are stored in `/var/apps/rayrag/var/rayrag.env` and are also editable from
the app settings (the App Center **Settings** page re-runs the wizard).

> Requirements: 自托管主机 1.1.3100 or newer, x86_64 device. The package bundles
> PostgreSQL 18.4, zvec and all runtime libraries it needs; it does not install a
> system database and does not require Docker.

## Where the data lives

| Path | Content |
| --- | --- |
| `/vol1/@appshare/rayrag/pgdata` | PostgreSQL 18 cluster (the database itself) |
| `/vol1/@appshare/rayrag/data` | RayRAG JSON stores, uploads, vector data, environment file copy |
| `/var/apps/rayrag/var/` | runtime state: `rayrag.env`, PID files, logs |
| `/var/apps/rayrag/var/rayrag.log` | RayRAG server log |
| `/var/apps/rayrag/var/postgres.log` | PostgreSQL log |

The `rayrag` share (second column above) appears in the 自托管主机 File Manager so backups are
simple: stop the app, back up the share, start the app again.

## Upgrade

Install the newer `.fpk` from the App Center the same way; the installer stops the old
version, keeps the data directory and PostgreSQL cluster, and starts the new version.
The version is shown on the app card.

## Uninstall

Uninstall from the App Center. The uninstaller stops both services. The `rayrag` share
under `/vol1/@appshare/rayrag` is kept so a later reinstall can reuse the data — delete it
manually if you want to remove everything.

## Build the FPK from source

You need: Rust (1.85+), `fnpack` (自托管主机 packaging CLI), `curl`, `unzip`, `tar`, `xz`,
`strip` and ImageMagick (only for regenerating icons).

```bash
# 1) fnpack (official CLI, linux-amd64)
curl -fL -o fnpack https://static2.fnnas.com/fnpack/fnpack-1.4.1-linux-amd64
chmod +x fnpack && sudo mv fnpack /usr/local/bin/fnpack

# 2) build the release binary once (PostgreSQL metadata + zvec backend)
cargo build --release --locked --features postgres-backend,zvec-backend

# 3) build the package; the script downloads the portable PostgreSQL 18.4
#    binaries from the Aliyun Maven mirror and bundles zvec automatically
./fpk/build-fpk.sh
# -> dist/rayrag_<version>_x86_64.fpk
```

The script is offline-friendly: pass `RAYRAG_FPK_PG_ARCHIVE=/path/postgres-linux-x86_64.txz`
when the NAS or build host already has the portable PostgreSQL archive cached, and
`RAYRAG_ZVEC_LIB=/path/libzvec_c_api.so` to bundle a different zvec build.
`RAYRAG_FPK_PG_URL` overrides the download mirror (default: Aliyun Maven, reachable in
mainland China).

### Package layout

```
fpk/
  manifest              app metadata (version is stamped by build-fpk.sh)
  ICON.PNG / ICON_256.PNG
  LICENSE
  config/privilege      runs as the dedicated package user
  config/resource       declares the "rayrag" data share
  cmd/main              start/stop/status (PostgreSQL + RayRAG)
  cmd/install_init ...  lifecycle scripts (install, upgrade, config, uninstall)
  cmd/rayrag-fpk-common shared helpers (paths, env file, DB + app lifecycle)
  wizard/install        PostgreSQL password prompt
  wizard/config         change the password later
  app/ui/config         desktop/iframe entry on port 9395
  app/ui/images/        entry icons
  build-fpk.sh          reproducible build: release binary + static UI + libzvec + PostgreSQL
```

## Troubleshooting

| Symptom | What to check |
| --- | --- |
| Card opens but the page cannot connect | `/var/apps/rayrag/var/rayrag.log` and the port in the manifest |
| App stops right after start | `/var/apps/rayrag/var/postgres.log` (database permissions/ports) |
| Forgotten PostgreSQL password | App Center → RayRAG → Settings → set a new password; the script updates the database role and restarts the app |
| Need a clean database | stop the app, delete `/vol1/@appshare/rayrag/pgdata`, reinstall the `.fpk` |

Lifecycle scripts log to `/var/apps/rayrag/var/rayrag-fpk.log`; the install log is
`/var/apps/rayrag/var/install.log`.
