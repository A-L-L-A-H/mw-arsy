# CLI Reference

The `arsy` CLI provides full command-line access to all panel operations. It communicates with the agent via Unix socket using the token stored at `/etc/arsy/agent.token`.

## Global Options

| Option | Default | Description |
|--------|---------|-------------|
| `-o, --output <FORMAT>` | `table` | Output format: `table` or `json` |
| `--version` | | Print version and exit |
| `--help` | | Print help |

## Commands

---

### `arsy status`

Show server status including CPU, memory, disk, and uptime.

```bash
arsy status
```

```
SERVER STATUS
─────────────────────────────────
Hostname:    web-1
OS:          Ubuntu 22.04.4 LTS
Kernel:      6.8.0-106-generic
Uptime:      14 days, 3 hours
Load:        0.12 0.08 0.05

CPU:         3.2% (2 cores)
Memory:      847 MB / 2048 MB (41.4%)
Disk:        12.3 GB / 50.0 GB (24.6%)
```

JSON output:

```bash
arsy status -o json
```

---

### `arsy sites`

List all Nginx sites.

```bash
arsy sites
```

```
DOMAIN                RUNTIME    SSL    STATUS
example.com           php        ✓      active
api.example.com       proxy      ✓      active
blog.example.com      static     ✓      active
```

Filter by domain:

```bash
arsy sites -f blog
```

#### `arsy sites create`

Create a new site.

```bash
arsy sites create example.com --runtime php --ssl --ssl-email admin@example.com
```

| Argument | Required | Default | Description |
|----------|----------|---------|-------------|
| `DOMAIN` | Yes | | Domain name |
| `--runtime` | No | `static` | Runtime type: `static`, `php`, or `proxy` |
| `--proxy-port` | No | | Upstream port (required for `--runtime proxy`) |
| `--ssl` | No | | Provision Let's Encrypt SSL |
| `--ssl-email` | No | | Email for Let's Encrypt (required with `--ssl`) |

```
Site created: example.com
  Runtime:  php
  Root:     /var/www/example.com/public
  SSL:      provisioned (expires 2026-06-18)
```

#### `arsy sites info`

Show site details.

```bash
arsy sites info example.com
```

```
SITE DETAILS
─────────────────────────────────
Domain:      example.com
Runtime:     php
Root:        /var/www/example.com/public
SSL:         active (expires 2026-06-18)
Created:     2026-03-15 10:30:00
```

#### `arsy sites delete`

Delete a site and its Nginx configuration.

```bash
arsy sites delete example.com
```

```
Site deleted: example.com
```

---

### `arsy db`

List databases.

```bash
arsy db
```

```
NAME              ENGINE      PORT    STATUS     SIZE
mysite_db         mysql       3306    running    245 MB
analytics_db      postgres    5433    running    1.2 GB
```

Filter by name:

```bash
arsy db -f analytics
```

#### `arsy db create`

Create a new database in a Docker container.

```bash
# Prefer --password-stdin — a bare --password is visible to other local
# users via `ps`/`/proc` for the life of the process and lands in shell
# history.
echo -n "s3cureP@ss" | arsy db create blog_db --engine mysql --password-stdin --port 3307

# Or omit both --password and --password-stdin for an interactive, masked prompt.
arsy db create blog_db --engine mysql --port 3307
```

| Argument | Required | Description |
|----------|----------|-------------|
| `NAME` | Yes | Database name |
| `--engine` | Yes | Engine: `mysql`, `mariadb`, or `postgres` |
| `--password` | No | Root/admin password. Discouraged — visible via shell history and `ps`. |
| `--password-stdin` | No | Read the root/admin password from stdin instead |
| `--port` | Yes | Host port to expose |

If neither `--password` nor `--password-stdin` is given, the CLI prompts interactively.

```
Database created: blog_db
  Engine:    mysql
  Port:      3307
  Container: arsy-db-blog_db
```

#### `arsy db delete`

Delete a database container.

```bash
arsy db delete abc123def456
```

---

### `arsy apps`

List Docker apps.

```bash
arsy apps
```

```
NAME           IMAGE                   PORT    STATUS     DOMAIN
ghost          ghost:5-alpine          2368    running    blog.example.com
grafana        grafana/grafana:latest  3000    running    metrics.example.com
n8n            n8nio/n8n:latest        5678    running    —
```

Filter by name or domain:

```bash
arsy apps -f grafana
```

#### `arsy apps templates`

List all available app templates.

```bash
arsy apps templates
```

```
ID                CATEGORY      NAME             DESCRIPTION
ghost             cms           Ghost            Modern publishing platform
wordpress         cms           WordPress        Popular CMS and blogging platform
grafana           monitoring    Grafana          Observability dashboards
prometheus        monitoring    Prometheus       Metrics collection
uptime-kuma       monitoring    Uptime Kuma      Uptime monitoring
nextcloud         storage       Nextcloud        Self-hosted cloud storage
...
(147 templates across 14 categories)
```

#### `arsy apps deploy`

Deploy an app from a template.

```bash
arsy apps deploy ghost --name my-blog --port 2368 --domain blog.example.com --ssl-email admin@example.com
```

| Argument | Required | Description |
|----------|----------|-------------|
| `TEMPLATE` | Yes | Template ID (from `apps templates`) |
| `--name` | Yes | App name |
| `--port` | Yes | Host port |
| `--domain` | No | Domain for auto reverse proxy + SSL |
| `--ssl-email` | No | Email for Let's Encrypt (requires `--domain`) |

```
Deploying ghost as "my-blog"...
  Pulling image: ghost:5-alpine
  Starting container on port 2368
  Configuring reverse proxy: blog.example.com → localhost:2368
  Provisioning SSL for blog.example.com
App deployed: my-blog (blog.example.com)
```

#### `arsy apps stop`

```bash
arsy apps stop abc123def456
```

#### `arsy apps start`

```bash
arsy apps start abc123def456
```

#### `arsy apps restart`

```bash
arsy apps restart abc123def456
```

#### `arsy apps remove`

```bash
arsy apps remove abc123def456
```

#### `arsy apps logs`

View container logs.

```bash
arsy apps logs abc123def456
```

#### `arsy apps compose`

Deploy from a Docker Compose file.

```bash
arsy apps compose /path/to/docker-compose.yml
```

A stack deployed this way carries no domain. To front a stack with a domain — over
Let's Encrypt or a registered certificate — create it through the panel
(`POST /api/stacks`), which records how the domain is served. Registered
certificates are managed from the panel, not the CLI: the CLI speaks to the agent
alone, and a certificate the panel's database does not know cannot be referenced by
a stack.

---

### `arsy services`

Check service health.

```bash
arsy services
```

```
SERVICE              STATUS      PID     MEMORY
arsy-agent      ● running   1234    30 MB
arsy-api        ● running   1235    27 MB
nginx                ● running   1236    12 MB
docker               ● running   1237    45 MB
php8.3-fpm           ● running   1238    18 MB
fail2ban             ● running   1239    8 MB
ufw                  ● active    —       —
```

Filter by service name:

```bash
arsy services -f nginx
```

---

### `arsy ssl`

SSL certificate management.

#### `arsy ssl status`

Check certificate details for a domain.

```bash
arsy ssl status example.com
```

```
SSL Certificate: example.com
  Issuer:      R11
  Expires:     2026-06-13 09:41:07.0 +00:00:00
  Remaining:   85 days
```

The days remaining are coloured: green above 30, amber above 7, red at or below.
A domain with no certificate prints `No SSL certificate for example.com`.

#### `arsy ssl provision`

Provision a Let's Encrypt certificate.

```bash
arsy ssl provision example.com --email admin@example.com --runtime php
```

| Argument | Required | Default | Description |
|----------|----------|---------|-------------|
| `DOMAIN` | Yes | | Domain name |
| `--email` | Yes | | Let's Encrypt email |
| `--runtime` | No | `static` | Site runtime: `static`, `php`, or `proxy` |
| `--proxy-port` | No | | Upstream port (for proxy runtime) |
| `--force` | No | | Issue even if the installed certificate came from elsewhere — it will be **replaced** |

**Ordering over somebody else's certificate is refused.** Provisioning writes
`fullchain.pem` and `privkey.pem` into `/etc/arsy/ssl/<domain>/`, so a
domain already serving a purchased, Origin CA or corporate PKI certificate would
have it destroyed rather than refreshed. The agent checks the installed
certificate's issuer first and refuses by name:

```
Refusing to issue a Let's Encrypt certificate for example.com: the certificate
already installed there was issued by DigiCert Inc, not by ARSY, and
ordering would overwrite it. Renew it wherever it was issued, or pass --force if
you intend to replace it.
```

A certificate whose issuer cannot be read is **not** treated as somebody else's —
refusing on doubt is how a real certificate lapses. The same guard covers
`arsy sites create --ssl` and `arsy apply`, because it lives at the
point where the file is written rather than at each command.

> There is no `arsy ssl renew`. Renewal is the panel's job — the weekly
> security scan reissues what it owns, including Compose stacks since 2.161.0.

---

### `arsy backup`

Backup management.

#### `arsy backup create`

```bash
arsy backup create example.com
```

```
Creating backup for example.com...
✓ Backup created
  File:    example.com-20260320-143022.tar.gz
  Size:    45.2 MB
  Content: files only

! This archive contains the site's files but NOT its databases.
  The CLI talks to the agent directly and cannot resolve a site's databases.
  Create the backup from the panel (or the panel API) to include them.
```

CLI backups never include databases — the agent has no access to the panel's database records.
Create the backup from the panel if you need one that holds the site's content.

#### `arsy backup list`

```bash
arsy backup list example.com
```

#### `arsy backup restore`

```bash
arsy backup restore example.com example.com_2026-03-20_143022.tar.gz
```

```
Restoring example.com from example.com_2026-03-20_143022.tar.gz...
✓ Backup restored
  Content: files only (this archive holds no database)
```

Restoring an archive that carries database dumps works from the CLI when a panel API key is
configured (`Settings → API Keys`, saved to `/etc/arsy/backend.token`) — `backup restore`
prefers that path automatically, restoring files and databases through the same route the panel
UI's own Restore button uses, then following its progress stream to completion.

Without a configured key, `backup restore` falls back to the older agent-direct path (files
only, same behavior as before this existed): it **fails** on a DB-bearing archive rather than
restoring the files and calling that success, and its error message points you at minting a key
or restoring from the panel instead.

#### `arsy backup delete`

```bash
arsy backup delete example.com example.com_2026-03-18_020000.tar.gz
```

---

### `arsy logs`

View system and site logs.

```bash
arsy logs
```

| Option | Default | Description |
|--------|---------|-------------|
| `-d, --domain` | | Domain for site-specific logs |
| `-t, --type` | `syslog` | Log type: `syslog`, `nginx`, `auth`, `php`, `mysql` |
| `-n, --lines` | `50` | Number of lines to show |
| `-f, --filter` | | Filter text (substring match) |
| `-s, --search` | | Search pattern (regex) |

Examples:

```bash
# View system log
arsy logs

# View Nginx error log for a site
arsy logs -d example.com -t nginx -n 100

# Search for errors in auth log
arsy logs -t auth -s "Failed password"

# Filter PHP logs
arsy logs -t php -f "Fatal error" -n 200
```

---

### `arsy security`

Security overview.

```bash
arsy security
```

```
Security Overview
  Firewall:    active
  Fail2ban:    active
  SSH root:    disabled
  SSH password: disabled
```

#### `arsy security scan`

Run a security scan.

```bash
arsy security scan
```

```
Running security scan...
Scan Results
  Risk level:  warning

  [warning] Unexpected open port: 3306
  [warning] SSH password authentication still enabled
  [info]    Unattended upgrades not configured

3 finding(s)
```

Risk level and each finding's severity are one of `critical`/`warning`/`info`,
derived from the findings the scan actually returns — there is no
separate numeric score.

#### `arsy security firewall`

List firewall rules.

```bash
arsy security firewall
```

```
#      TO           ACTION     FROM
1      22/tcp       ALLOW IN   Anywhere
2      80/tcp       ALLOW IN   Anywhere
3      443/tcp      ALLOW IN   Anywhere
4      8443/tcp     ALLOW IN   Anywhere
```

#### `arsy security firewall add`

Add a firewall rule.

```bash
arsy security firewall add --port 3000 --proto tcp --action allow
arsy security firewall add --port 5432 --proto tcp --action allow --from 10.0.0.0/8
```

| Option | Default | Description |
|--------|---------|-------------|
| `--port` | | Port number |
| `--proto` | `tcp` | Protocol: `tcp` or `udp` |
| `--action` | `allow` | Action: `allow` or `deny` |
| `--from` | | Source IP or CIDR (optional) |

#### `arsy security firewall remove`

Remove a rule by number.

```bash
arsy security firewall remove 4
```

---

### `arsy top`

Show top processes by CPU usage.

```bash
arsy top
```

```
PID      CPU%    MEM%    COMMAND
1234     12.3    2.1     /usr/sbin/mysqld
5678     8.7     1.4     php-fpm: pool www
9012     3.2     0.8     nginx: worker process
1357     2.1     1.2     arsy-agent
2468     1.8     1.1     arsy-api
```

---

### `arsy php`

PHP version management.

```bash
arsy php
```

```
VERSION    STATUS     FPM SOCKET
8.1        installed  /run/php/php8.1-fpm.sock
8.3        installed  /run/php/php8.3-fpm.sock
```

#### `arsy php install`

Install a PHP version.

```bash
arsy php install 8.4
```

Supported versions: `8.1`, `8.2`, `8.3`, `8.4`, `8.5`.

---

### `arsy diagnose`

Run server diagnostics across 6 categories.

```bash
arsy diagnose
```

```
DIAGNOSTICS
─────────────────────────────────
[✓] Nginx configuration valid
[✓] All SSL certificates valid (next expiry: 85 days)
[✓] Disk usage: 24.6% (12.3 GB / 50 GB)
[✓] Memory usage: 41.4% (847 MB / 2048 MB)
[✓] Docker: 5 containers running, 0 unhealthy
[!] PHP-FPM: high average response time (320ms)
[✓] Fail2Ban: 3 jails active
[✓] Firewall: active

Score: 95/100 (1 warning)
```

---

### `arsy export`

Export server configuration as YAML (Infrastructure as Code).

```bash
# Print to stdout
arsy export

# Save to file
arsy export -O config.yml
```

Sample output:

```yaml
version: "1"
sites:
  - domain: example.com
    runtime: php
    ssl: true
  - domain: api.example.com
    runtime: proxy
    proxy_port: 3000
    ssl: true
databases:
  - name: mysite_db
    engine: mysql
    port: 3306
apps:
  - name: ghost
    template: ghost
    port: 2368
    domain: blog.example.com
```

---

### `arsy apply`

Apply server configuration from a YAML file.

```bash
# Dry run (show what would change)
arsy apply config.yml --dry-run

# Apply changes
arsy apply config.yml --email admin@example.com
```

| Argument | Required | Description |
|----------|----------|-------------|
| `FILE` | Yes | Path to YAML config file |
| `--dry-run` | No | Show changes without applying |
| `--email` | No | Email for Let's Encrypt SSL provisioning |

`apply` only creates resources that are missing — for sites, databases, apps and
PHP versions it never diffs or updates something that already exists, even if the
file describes it differently now (e.g. a changed `proxy_port` or `php_version`
on an existing site is silently ignored). Cron jobs are the one exception: a
matching `id` already in the crontab is compared and re-synced if its schedule or
command changed. Firewall rules in an exported file are shown for reference only —
`apply` never creates, updates, or removes them; manage those with
`arsy security firewall`.

Dry run output:

```
Plan:
  + site: staging.example.com (static)
  = site: api.example.com (already exists)
  + database: staging_db (postgres, port 5434)
  + cron: nightly-backup (0 2 * * * /opt/scripts/backup.sh)
  = cron: log-rotate (already exists, unchanged)

3 to create, 2 existing
```

---

### `arsy completions`

Generate shell completions.

```bash
# Bash
arsy completions bash > /etc/bash_completion.d/arsy

# Zsh
arsy completions zsh > ~/.zfunc/_arsy

# Fish
arsy completions fish > ~/.config/fish/completions/arsy.fish
```

Supported shells: `bash`, `zsh`, `fish`, `powershell`, `elvish`.
