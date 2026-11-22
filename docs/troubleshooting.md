# Troubleshooting

## Log Locations

ARSY services run via systemd. View their logs with `journalctl`:

```bash
# Agent logs
journalctl -u arsy-agent -n 100 --no-pager

# API logs
journalctl -u arsy-api -n 100 --no-pager

# Follow logs in real-time
journalctl -u arsy-agent -f

# Logs since last boot
journalctl -u arsy-agent -b
```

For verbose output, set the log level in the service config:

```bash
# Edit /etc/arsy/api.env and add:
RUST_LOG=debug

# Then restart:
systemctl restart arsy-api
```

### Other useful logs

```bash
# Nginx access/error logs
tail -50 /var/log/nginx/error.log
tail -50 /var/log/nginx/access.log

# Per-site Nginx logs
tail -50 /var/log/nginx/example.com.access.log
tail -50 /var/log/nginx/example.com.error.log

# PHP-FPM
journalctl -u php8.3-fpm -n 50 --no-pager

# Fail2Ban
tail -50 /var/log/fail2ban.log

# Mail (Postfix)
tail -50 /var/log/mail.log
```

## Common Issues

### Panel shows 502 Bad Gateway

**Cause**: The ARSY agent is not running.

**Fix**:

```bash
# Check agent status
systemctl status arsy-agent

# Start it if stopped
systemctl start arsy-agent

# Check for errors
journalctl -u arsy-agent -n 50 --no-pager
```

Also check the API service:

```bash
systemctl status arsy-api
journalctl -u arsy-api -n 50 --no-pager
```

---

### Can't login from browser (cookie not set)

**Cause**: The login cookie carries the `Secure` flag, which browsers only return over HTTPS, while the panel is being served over plain HTTP.

**Fix**: serve the panel over HTTPS — `BASE_URL` has nothing to do with it.

The `Secure` flag is decided by the scheme nginx reports for the actual connection
(`X-Forwarded-Proto`), not by any setting. Keying it off `BASE_URL` was the cause of issue
#71 and was removed: `BASE_URL` is `https://<domain>` as soon as a domain is configured,
which forced a `Secure` cookie the browser then dropped whenever the vhost was still
answering over HTTP. Editing `BASE_URL` will not change the cookie.

Installs from v2.31.0 onward do not hit this: with a domain the panel gets a Let's Encrypt
certificate, and without one it gets a self-signed certificate, so the panel is served over
HTTPS either way. If you are on an older install serving plain HTTP, either re-run the
installer with `PANEL_DOMAIN=your.domain`, or put a certificate in front of the panel vhost
(`/etc/nginx/sites-available/arsy-panel`).

`BASE_URL` is still worth setting correctly — it is what the panel uses to build links in
outgoing notifications and runbooks — it simply is not what gates the cookie.

---

### Site shows 404 Not Found

**Cause**: Either the document root is empty, or the DNS is not pointing to the server.

**Fix**:

1. Check the document root exists and has files:
   ```bash
   ls -la /var/www/example.com/public/
   ```

2. If the directory is empty, ARSY should have created a default `index.html`. Re-create the site from the panel.

3. Check DNS is pointing to this server:
   ```bash
   dig example.com +short
   ```
   The result should be your server's public IP.

4. Check the Nginx config exists and is valid:
   ```bash
   ls /etc/nginx/sites-enabled/example.com
   nginx -t
   ```

---

### PHP site shows 502 Bad Gateway

**Cause**: PHP-FPM is not installed or not running.

**Fix**:

```bash
# Check if PHP-FPM is installed
php -v

# Check the service
systemctl status php8.3-fpm

# Install PHP if missing (via CLI)
arsy php install 8.3

# Or install from the panel: Settings > Service Installers > PHP-FPM

# Restart PHP-FPM
systemctl restart php8.3-fpm
```

Verify the Nginx config references the correct PHP-FPM socket:

```bash
grep fastcgi_pass /etc/nginx/sites-available/example.com
# Should show: unix:/run/php/php8.3-fpm.sock
```

---

### SSL provisioning fails

**Cause**: Let's Encrypt cannot verify domain ownership.

Before working through this list, check the SSL card on the site itself. ARSY evaluates the DNS prerequisite before it will attempt issuance, and says which of these it is — see [What ARSY checks for you](guides/prerequisites.md). If the button is disabled, the reason is on screen.

**Fix**:

1. **DNS must resolve**:
   ```bash
   dig example.com +short
   # Must return something
   ```

   Usually this is the server's own public IP. It does **not** have to be: a
   domain behind Cloudflare's proxy resolves to Cloudflare's addresses and
   issuance still succeeds, because the proxy passes the challenge request
   through to this server. A domain that resolves to *nothing* is the case that
   cannot work.

2. **Port 80 must be open** (HTTP-01 challenge):
   ```bash
   ufw status | grep 80
   # Should show: 80/tcp ALLOW Anywhere

   # Open it if blocked:
   ufw allow 80/tcp
   ```

3. **Nginx must be running**:
   ```bash
   systemctl status nginx
   ```

4. **Check for rate limits**: Let's Encrypt allows 5 duplicate certificates per domain per week. If you hit the limit, wait 7 days or use a different subdomain.

5. Retry:
   ```bash
   arsy ssl provision example.com --email you@example.com --runtime static
   ```

---

### Agent won't start

**Cause**: Missing directories, port conflicts, or permission issues.

**Fix**:

```bash
# Check the error message
journalctl -u arsy-agent -n 50 --no-pager

# Ensure required directories exist
mkdir -p /run/arsy
mkdir -p /etc/arsy
mkdir -p /var/backups/arsy
mkdir -p /var/www/acme

# Check the agent token exists
ls -la /etc/arsy/agent.token

# Restart
systemctl restart arsy-agent
```

If you see `NAMESPACE` errors in the logs, the systemd service may have stale hardening directives. Update the service file:

```bash
sudo bash /opt/arsy/scripts/update.sh
```

---

### After reboot, services are down

**Cause**: Systemd services were not enabled to start on boot.

**Fix**:

```bash
# Enable all ARSY services
systemctl enable arsy-agent
systemctl enable arsy-api

# Start them now
systemctl start arsy-agent
systemctl start arsy-api

# Verify
systemctl status arsy-agent
systemctl status arsy-api
```

Also ensure Docker is enabled:

```bash
systemctl enable docker
systemctl start docker
```

---

### Docker apps not accessible after deploy

**Cause**: The container is running but the reverse proxy is not configured, or the container port is wrong.

**Fix**:

```bash
# Check the container is running
docker ps | grep APP_NAME

# Check the container's port mapping
docker port CONTAINER_ID

# Check Nginx config exists
ls /etc/nginx/sites-enabled/ | grep APP_DOMAIN

# Test Nginx config
nginx -t

# Reload Nginx
systemctl reload nginx
```

## Running Diagnostics

### From the CLI

```bash
arsy diagnose
```

This checks 6 categories:
- Nginx configuration validity
- Resource usage (CPU, RAM, disk)
- SSL certificate expiry
- Security configuration
- Service health
- Log analysis for errors

### From the Panel

The Dashboard shows a health score (0-100) with active issues. Click any issue to see details and one-click fix options.

Go to Security > Diagnostics for a full diagnostic report.

## Restarting Services

```bash
# Restart all ARSY services
systemctl restart arsy-agent
systemctl restart arsy-api

# Restart Nginx
systemctl restart nginx

# Restart PHP-FPM
systemctl restart php8.3-fpm

# Restart Docker
systemctl restart docker

# Restart a specific Docker container
docker restart CONTAINER_ID
```

## Updating ARSY

```bash
sudo bash /opt/arsy/scripts/update.sh
```

The update script pulls the latest code, rebuilds binaries, updates systemd service files, creates any new required directories, and restarts services with zero-downtime rollback if the health check fails.

For servers using pre-built binaries:

```bash
INSTALL_FROM_RELEASE=1 sudo bash /opt/arsy/scripts/update.sh
```

## Getting Help

- **GitHub Issues**: [github.com/A-L-L-A-H/mw-arsy/issues](https://github.com/A-L-L-A-H/mw-arsy/issues) -- Report bugs or request features
- **Discussions**: Use GitHub Issues for questions and community support
- **Email**: hello@moonwitness.dev for priority support inquiries
