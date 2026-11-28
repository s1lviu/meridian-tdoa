#!/bin/sh
# One-time, idempotent preparation of a shared Ubuntu host that already runs Apache with certbot.
# Run as root:  sudo DOMAIN=meridian.example.org sh deploy/host/setup.sh
#   DOMAIN       public hostname (required)
#   PORT         local port of the service, bound to 127.0.0.1 (default 8790)
#   DATA_SIZE    size of the dedicated data filesystem (default 10G)
#   DEPLOY_USER  user that runs deployments, member of the docker group (default: $SUDO_USER)
#
# The application listens on 127.0.0.1 only. Its data lives on a fixed-size ext4 image, so it
# cannot fill the host disk. Apache proxies to it on port 80 and, once certbot has issued a
# certificate, on 443. Port 80 never redirects: with Cloudflare's Flexible SSL mode Cloudflare
# reaches the origin over HTTP, and a redirect would loop. Cloudflare redirects visitors to HTTPS.
set -eu
domain=${DOMAIN:?Set DOMAIN, e.g. DOMAIN=meridian.example.org}
port=${PORT:-8790}
size=${DATA_SIZE:-10G}
user=${DEPLOY_USER:-${SUDO_USER:-}}
[ "$(id -u)" = 0 ] || { echo "Run as root (sudo)." >&2; exit 1; }
[ -n "$user" ] && id -nG "$user" | grep -qw docker || { echo "DEPLOY_USER must exist and be in the docker group." >&2; exit 1; }
printf '%s' "$domain" | grep -Eq '^[a-z0-9][a-z0-9.-]+\.[a-z]{2,}$' || { echo "Invalid DOMAIN." >&2; exit 1; }
for tool in docker apache2ctl certbot mkfs.ext4 fallocate mountpoint; do
    command -v "$tool" >/dev/null || { echo "Missing $tool." >&2; exit 1; }
done
if ss -tlnH "sport = :$port" | grep -q . && ! docker ps --format '{{.Ports}}' | grep -q "127.0.0.1:$port->"; then
    echo "Port $port is used by another service; choose PORT." >&2; exit 1
fi

root=/opt/meridian data=/srv/meridian
install -d -o "$user" -g "$user" -m 0750 "$root" "$root/releases"
install -d -o "$user" -g "$user" -m 0700 "$root/.secrets"
if [ ! -s "$root/.secrets/runtime.env" ]; then
    (umask 077; printf 'MERIDIAN_ADMIN_TOKEN=%s\n' "$(openssl rand -hex 32)" > "$root/.secrets/runtime.env")
    chown "$user:$user" "$root/.secrets/runtime.env"
fi
cat > "$root/.env" <<EOF
MERIDIAN_DATA_DIR=$data/data/app
MERIDIAN_PORT=$port
MERIDIAN_MEMORY=3g
MERIDIAN_CPUS=2
EOF
chown "$user:$user" "$root/.env"

# Fixed-size data filesystem (loop-mounted ext4 image), mounted at boot.
install -d -m 0755 "$data" "$data/data"
if [ ! -f "$data/data.img" ]; then
    fallocate -l "$size" "$data/data.img"
    chmod 0600 "$data/data.img"
    mkfs.ext4 -q -F -m 0 -L meridian-data "$data/data.img"
fi
grep -qs "^$data/data.img " /etc/fstab ||
    echo "$data/data.img $data/data ext4 loop,nofail,noatime,nodev,nosuid,noexec 0 2" >> /etc/fstab
mountpoint -q "$data/data" || mount "$data/data"
install -d -o 10001 -g 10001 -m 0750 "$data/data/app"

# Apache: a site for $domain. Every change is checked with configtest before a graceful reload.
install -d -m 0755 /var/www/letsencrypt
proxy() {
    cat <<EOF
    ServerName $domain
    ProxyPreserveHost On
    # certbot renews through this path; everything else goes to Meridian on 127.0.0.1:$port.
    Alias /.well-known/acme-challenge/ /var/www/letsencrypt/.well-known/acme-challenge/
    <Directory /var/www/letsencrypt>
        Require all granted
    </Directory>
    ProxyPass /.well-known/acme-challenge/ !
    # Server-sent events: flush each event, keep the stream open, never compress it.
    ProxyPass /api/stream http://127.0.0.1:$port/api/stream flushpackets=on timeout=86400 retry=0 connectiontimeout=5
    ProxyPassReverse /api/stream http://127.0.0.1:$port/api/stream
    SetEnvIf Request_URI "^/api/stream" no-gzip dont-vary
    ProxyPass / http://127.0.0.1:$port/ retry=0 connectiontimeout=5 timeout=300
    ProxyPassReverse / http://127.0.0.1:$port/
    LimitRequestBody 262144
    ErrorLog \${APACHE_LOG_DIR}/meridian_error.log
    CustomLog \${APACHE_LOG_DIR}/meridian_access.log combined
EOF
}
# A rejected configuration disables the new site at once, so the other sites keep working.
reload() {
    if apache2ctl configtest >/dev/null 2>&1; then
        systemctl reload apache2
    else
        a2dissite -q "$1" >/dev/null
        echo "Apache rejected $1; the site was disabled and Apache was not reloaded." >&2
        exit 1
    fi
}
{ echo "<VirtualHost *:80>"; proxy; echo "</VirtualHost>"; } > "/etc/apache2/sites-available/$domain.conf"
a2ensite -q "$domain.conf" >/dev/null
reload "$domain.conf"

if [ ! -d "/etc/letsencrypt/live/$domain" ]; then
    certbot certonly --webroot -w /var/www/letsencrypt -d "$domain" --non-interactive \
        --keep-until-expiring --deploy-hook "systemctl reload apache2" ||
        echo "certbot failed: the site is served on port 80 only (enough for Cloudflare Flexible)." >&2
fi
if [ -d "/etc/letsencrypt/live/$domain" ]; then
    {
        echo "<IfModule mod_ssl.c>"
        echo "<VirtualHost *:443>"
        proxy
        echo "    SSLCertificateFile /etc/letsencrypt/live/$domain/fullchain.pem"
        echo "    SSLCertificateKeyFile /etc/letsencrypt/live/$domain/privkey.pem"
        echo "    Include /etc/letsencrypt/options-ssl-apache.conf"
        echo "</VirtualHost>"
        echo "</IfModule>"
    } > "/etc/apache2/sites-available/$domain-le-ssl.conf"
    a2ensite -q "$domain-le-ssl.conf" >/dev/null
    reload "$domain-le-ssl.conf"
fi
echo "Host ready: $domain -> 127.0.0.1:$port, data on $(df -h --output=size "$data/data" | tail -1 | tr -d ' ') at $data/data."
