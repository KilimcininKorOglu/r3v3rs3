#!/usr/bin/env bash
# Installs or upgrades r3v3rs3 from a GitHub release and runs it as a systemd service.
#
#   curl -fsSL https://raw.githubusercontent.com/KilimcininKorOglu/r3v3rs3/main/install.sh | sudo bash
#   curl -fsSL https://raw.githubusercontent.com/KilimcininKorOglu/r3v3rs3/main/install.sh | sudo bash -s -- --version 1.0.1 --webui 0.0.0.0:46492
#   curl -fsSL https://raw.githubusercontent.com/KilimcininKorOglu/r3v3rs3/main/install.sh | sudo bash -s -- --password s3cret-PASSWORD
#   curl -fsSL https://raw.githubusercontent.com/KilimcininKorOglu/r3v3rs3/main/install.sh | sudo bash -s -- --agent --master master.example.com:9443 --token TOKEN
set -euo pipefail

REPO="KilimcininKorOglu/r3v3rs3"
BIN="/usr/local/bin/r3v3rs3"
CONFIG_DIR="/etc/r3v3rs3"
LOG_DIR="/var/log/r3v3rs3"
UNIT="/etc/systemd/system/r3v3rs3.service"
DEFAULT_WEBUI="127.0.0.1:46492"
AGENT_UNIT="/etc/systemd/system/r3v3rs3-agent.service"
AGENT_DATA_DIR="/var/lib/r3v3rs3-agent"
# The token is readable only by root and lives only until the enrollment.
AGENT_TOKEN_FILE="$AGENT_DATA_DIR/token.env"
# The files of an enrolled agent. The target file marks a complete identity.
AGENT_IDENTITY_FILES="agent.key agent.pem ca.pem target"

version=""
webui=""
password=""
agent=false
master=""
token=""

usage() {
	cat <<EOF
Usage: install.sh [--version X.Y.Z] [--webui ADDR] [--password PASSWORD]
       install.sh --agent [--version X.Y.Z] [--master HOST:PORT] [--token TOKEN]

  --version X.Y.Z    Install this release. The default is the latest release.
  --webui ADDR       The admin WebUI address, for example 127.0.0.1:46492 or 0.0.0.0:46492.
                     Without this option the script asks for it. The default is the address of
                     the installed service, or $DEFAULT_WEBUI.
  --password PASS    The password of the admin account that the install creates. Without this
                     option the script generates a strong random password and prints it.
  --agent            Install the agent of a deployment platform target instead of the server.
  --master HOST:PORT The agent port of the master. An upgrade keeps the installed address.
  --token TOKEN      The enrollment token of the target from the Targets page. A new token
                     enrolls the agent again. An upgrade of an enrolled agent needs no token.
EOF
}

die() {
	echo "error: $*" >&2
	exit 1
}

info() {
	echo "==> $*"
}

warn() {
	echo "warning: $*" >&2
}

# Reads from the terminal, so the prompts also work when the script comes through a pipe.
has_tty() {
	[ -c /dev/tty ] && { : </dev/tty; } 2>/dev/null
}

ask() {
	local prompt=$1 default=$2 answer=""
	if has_tty; then
		read -r -p "$prompt [$default]: " answer </dev/tty
	fi
	printf '%s' "${answer:-$default}"
}

parse_args() {
	while [ $# -gt 0 ]; do
		case "$1" in
		--version)
			[ $# -ge 2 ] || die "--version needs a value"
			version="${2#v}"
			shift 2
			;;
		--webui)
			[ $# -ge 2 ] || die "--webui needs a value"
			webui="$2"
			shift 2
			;;
		--password)
			[ $# -ge 2 ] || die "--password needs a value"
			password="$2"
			shift 2
			;;
		--agent)
			agent=true
			shift
			;;
		--master)
			[ $# -ge 2 ] || die "--master needs a value"
			master="$2"
			shift 2
			;;
		--token)
			[ $# -ge 2 ] || die "--token needs a value"
			token="$2"
			shift 2
			;;
		-h | --help)
			usage
			exit 0
			;;
		*) die "unknown option: $1" ;;
		esac
	done
	if [ "$agent" = true ]; then
		[ -z "$webui" ] || die "--webui is an option of the server, not of the agent"
		[ -z "$password" ] || die "--password is an option of the server, not of the agent"
	else
		[ -z "$master$token" ] || die "--master and --token need --agent"
	fi
}

check_system() {
	[ "$(id -u)" -eq 0 ] || die "run the script as root"
	[ "$(uname -s)" = "Linux" ] || die "r3v3rs3 supports only Linux"
	[ -d /run/systemd/system ] || die "systemd is not running on this system"
	local tool
	for tool in curl tar xz sha256sum systemctl; do
		command -v "$tool" >/dev/null || die "$tool is required"
	done
}

detect_target() {
	local arch
	case "$(uname -m)" in
	x86_64 | amd64) arch=x86_64 ;;
	aarch64 | arm64) arch=aarch64 ;;
	*) die "no release binary exists for $(uname -m); use x86_64 or aarch64" ;;
	esac
	# A musl system cannot run the glibc binary, so the static build is preferred there.
	if ls /lib/ld-musl-*.so.1 >/dev/null 2>&1; then
		echo "$arch-unknown-linux-musl"
	else
		echo "$arch-unknown-linux-gnu"
	fi
}

fetch_release_json() {
	local api="https://api.github.com/repos/$REPO/releases/latest"
	if [ -n "$version" ]; then
		api="https://api.github.com/repos/$REPO/releases/tags/v$version"
	fi
	curl -fsSL -H "Accept: application/vnd.github+json" "$api" || die "cannot read the release from $api"
}

# Prints the sha256 digest that GitHub stores for the named release asset.
asset_digest() {
	local json=$1 file=$2
	printf '%s' "$json" |
		grep -oE '"(name|digest)": *"[^"]*"' |
		awk -F'"' -v f="$file" '$2 == "name" { hit = ($4 == f); next } $2 == "digest" && hit { sub(/^sha256:/, "", $4); print $4; exit }'
}

install_binary() {
	local target=$1 json tag file digest tmp
	json=$(fetch_release_json)
	tag=$(printf '%s' "$json" | grep -oE '"tag_name": *"[^"]*"' | awk -F'"' '{ print $4; exit }')
	[ -n "$tag" ] || die "the release information has no tag"
	file="r3v3rs3-$target.tar.xz"
	digest=$(asset_digest "$json" "$file")
	if [ -z "$digest" ] && [ "$target" != "${target%-musl}" ]; then
		# The release of an older version has no musl archive; the glibc build still runs there.
		warn "release $tag has no $file; falling back to the glibc build"
		install_binary "${target%-musl}-gnu"
		return
	fi
	[ -n "$digest" ] || die "release $tag has no sha256 digest for $file"

	tmp=$(mktemp -d)
	# shellcheck disable=SC2064 # expand the path now, the variable is local
	trap "rm -rf '$tmp'" EXIT
	info "Downloading r3v3rs3 $tag for $target"
	curl -fsSL -o "$tmp/$file" "https://github.com/$REPO/releases/download/$tag/$file"
	echo "$digest  $tmp/$file" | sha256sum -c --quiet - || die "the sha256 digest of $file does not match"
	tar -xJf "$tmp/$file" -C "$tmp" r3v3rs3

	# Replace the file with a rename, because the running service holds the old binary open.
	install -m 0755 "$tmp/r3v3rs3" "$BIN.new"
	mv -f "$BIN.new" "$BIN"
	"$BIN" --help >/dev/null || die "$BIN does not start on this system; the release needs a newer glibc"
	installed_tag=$tag
}

choose_webui() {
	local current=""
	if [ -f "$UNIT" ]; then
		current=$(awk -F= '$1 == "Environment" && $2 == "R3V3RS3_WEBUI" { print $3 }' "$UNIT")
	fi
	if [ -z "$webui" ]; then
		webui=$(ask "Admin WebUI address" "${current:-$DEFAULT_WEBUI}")
	fi
	[[ "$webui" =~ ^([0-9.]+|\[[0-9a-fA-F:.]+\]):([0-9]+)$ ]] || die "the WebUI address must be IP:PORT, for example $DEFAULT_WEBUI"
	local port=${BASH_REMATCH[2]}
	[ "$port" -ge 1 ] && [ "$port" -le 65535 ] || die "the WebUI port must be between 1 and 65535"
}

# Generates a strong random password. base64 of 48 random bytes, stripped of the
# punctuation that a terminal or a password manager may mangle, cut to 24 characters.
generate_password() {
	local raw
	raw=$(head -c 48 /dev/urandom | base64 | tr -d '/+=')
	printf '%s' "${raw:0:24}"
}

# Creates the admin account as the last step of an install. An upgrade keeps the
# existing accounts and creates no account.
create_admin() {
	if [ -s "$CONFIG_DIR/accounts.toml" ]; then
		return
	fi
	if [ -z "$password" ]; then
		password=$(generate_password)
	fi
	if ! "$BIN" add-user --config-dir "$CONFIG_DIR" --password "$password" admin </dev/null; then
		die "cannot create the admin account"
	fi
	account_created=true
}

write_unit() {
	cat >"$UNIT" <<EOF
[Unit]
Description=r3v3rs3 reverse proxy
Documentation=https://r3v3rs3.keremgok.tr/
Wants=network-online.target
After=network-online.target

[Service]
Type=notify
NotifyAccess=main
Environment=R3V3RS3_CONFIG_DIR=$CONFIG_DIR
Environment=R3V3RS3_LOG_DIR=$LOG_DIR
Environment=R3V3RS3_WEBUI=$webui
ExecStart=$BIN start
# SIGUSR2 hands the listeners to a new process without dropping the connections.
ExecReload=/bin/kill -USR2 $MAINPID
# The server shuts down gracefully on SIGINT.
KillSignal=SIGINT
Restart=on-failure
RestartSec=5
LimitNOFILE=1048576

[Install]
WantedBy=multi-user.target
EOF
}

wait_for_webui() {
	local host=${webui%:*} port=${webui##*:} attempt
	case "$host" in
	0.0.0.0) host="127.0.0.1" ;;
	"[::]") host="[::1]" ;;
	esac
	for attempt in $(seq 1 30); do
		# Connection errors are expected until the service listens.
		if curl -fs -o /dev/null "http://$host:$port/"; then
			return
		fi
		sleep 1
	done
	systemctl --no-pager status r3v3rs3 || true
	journalctl --no-pager -u r3v3rs3 -n 50 || true
	die "the WebUI did not answer on http://$host:$port/ after $attempt seconds"
}

# Inserts one accept rule for the WebUI port into the INPUT chain of one iptables
# backend, and keeps the rule over a reboot. A no-op when the tool is missing, when a
# rule already accepts the port, and when no catch-all REJECT or DROP blocks it.
open_iptables() {
	local cmd=$1 conf=$2 port=$3 rules
	command -v "$cmd" >/dev/null 2>&1 || return 0
	rules=$("$cmd" -S INPUT 2>/dev/null) || return 0
	if printf '%s\n' "$rules" | grep -q -- "--dport $port "; then
		return 0
	fi
	# A policy other than ACCEPT, or a REJECT or DROP rule, needs an explicit accept.
	if printf '%s\n' "$rules" | grep -Eq -- '^(-P INPUT (DROP|REJECT)|-A INPUT .*(-j|--jump) (REJECT|DROP))'; then
		if "$cmd" -I INPUT -p tcp -m state --state NEW -m tcp --dport "$port" -j ACCEPT; then
			info "Opened port $port/tcp in $cmd"
		else
			warn "$cmd did not open port $port; open it by hand"
			return 0
		fi
	fi
	if command -v netfilter-persistent >/dev/null 2>&1; then
		netfilter-persistent save >/dev/null 2>&1
	elif [ -d "$conf" ]; then
		"$cmd"-save >"$conf/rules.v4"
	else
		warn "the $cmd rule does not survive a reboot; no persistence tool found"
	fi
}

# Opens the WebUI port in the local firewall. Cloud images such as the Oracle Ubuntu
# one ship a firewall that rejects every port no rule accepts, so a WebUI bound beyond
# the loopback interface stays unreachable without this step.
open_firewall() {
	local host=${webui%:*} port=${webui##*:}
	case "$host" in
	127.* | ::1 | "[::1]") return 0 ;;
	esac
	if command -v firewall-cmd >/dev/null 2>&1 && firewall-cmd --state >/dev/null 2>&1; then
		if firewall-cmd --add-port="$port/tcp" >/dev/null && firewall-cmd --runtime-to-permanent >/dev/null; then
			info "Opened port $port/tcp in firewalld"
		else
			warn "firewalld did not open port $port; open it by hand"
		fi
	elif command -v ufw >/dev/null 2>&1 && ufw status 2>/dev/null | grep -q "Status: active"; then
		if ufw allow "$port/tcp" >/dev/null; then
			info "Opened port $port/tcp in ufw"
		else
			warn "ufw did not open port $port; open it by hand"
		fi
	else
		open_iptables iptables /etc/iptables "$port"
		case "$host" in
		"["*) open_iptables ip6tables /etc/iptables "$port" ;;
		esac
	fi
}

check_docker() {
	[ -S /var/run/docker.sock ] || die "the agent runs the apps with Docker Engine, and /var/run/docker.sock does not exist"
	if ! docker compose version >/dev/null 2>&1; then
		echo "warning: the docker compose plugin is missing, so Compose apps cannot run on this target" >&2
	fi
}

choose_master() {
	local current=""
	if [ -f "$AGENT_UNIT" ]; then
		current=$(awk -F= '$1 == "Environment" && $2 == "R3V3RS3_AGENT_MASTER" { print $3 }' "$AGENT_UNIT")
	fi
	if [ -z "$master" ]; then
		master=$(ask "Agent port of the master (HOST:PORT)" "$current")
	fi
	[[ "$master" =~ ^([A-Za-z0-9.-]+|\[[0-9a-fA-F:.]+\]):([0-9]+)$ ]] || die "the master address must be HOST:PORT, for example master.example.com:9443"
	local port=${BASH_REMATCH[2]}
	[ "$port" -ge 1 ] && [ "$port" -le 65535 ] || die "the agent port must be between 1 and 65535"
}

is_enrolled() {
	[ -f "$AGENT_DATA_DIR/target" ]
}

# An enrolled agent needs no token. A token enrolls the agent again.
choose_token() {
	if [ -z "$token" ] && ! is_enrolled && has_tty; then
		read -r -s -p "Enrollment token: " token </dev/tty
		echo >/dev/tty
	fi
	if [ -z "$token" ]; then
		is_enrolled || die "the agent is not enrolled; pass the token of its target with --token"
		return
	fi
	[[ "$token" =~ ^[A-Za-z0-9]{43}\.[0-9a-f]{64}$ ]] || die "the token must be the enrollment token of the Targets page"
}

# Moves the identity of an earlier enrollment aside, so that the agent enrolls with the new token.
set_aside_identity() {
	local file
	for file in $AGENT_IDENTITY_FILES; do
		if [ -f "$AGENT_DATA_DIR/$file" ]; then
			mv -f "$AGENT_DATA_DIR/$file" "$AGENT_DATA_DIR/$file.old"
		fi
	done
}

restore_identity() {
	local file
	for file in $AGENT_IDENTITY_FILES; do
		if [ -f "$AGENT_DATA_DIR/$file.old" ]; then
			mv -f "$AGENT_DATA_DIR/$file.old" "$AGENT_DATA_DIR/$file"
		fi
	done
}

drop_old_identity() {
	local file
	for file in $AGENT_IDENTITY_FILES; do
		rm -f "$AGENT_DATA_DIR/$file.old"
	done
}

write_token_file() {
	(
		umask 077
		printf 'R3V3RS3_AGENT_TOKEN=%s\n' "$token" >"$AGENT_TOKEN_FILE"
	)
}

write_agent_unit() {
	cat >"$AGENT_UNIT" <<EOF
[Unit]
Description=r3v3rs3 agent of a deployment platform target
Documentation=https://r3v3rs3.keremgok.tr/platform/
Wants=network-online.target
After=network-online.target docker.service

[Service]
Type=simple
Environment=R3V3RS3_AGENT_MASTER=$master
Environment=R3V3RS3_AGENT_DATA_DIR=$AGENT_DATA_DIR
# Holds the enrollment token until the agent is enrolled.
EnvironmentFile=-$AGENT_TOKEN_FILE
ExecStart=$BIN agent
# The agent closes its connection gracefully on SIGINT.
KillSignal=SIGINT
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
EOF
}

agent_failed() {
	systemctl --no-pager status r3v3rs3-agent || true
	journalctl --no-pager -u r3v3rs3-agent -n 50 || true
	die "$*"
}

# Waits until the agent stores the identity of its target.
wait_for_enrollment() {
	local attempt
	for attempt in $(seq 1 30); do
		if is_enrolled; then
			return 0
		fi
		sleep 1
	done
	return 1
}

start_agent() {
	systemctl daemon-reload
	systemctl enable r3v3rs3-agent >/dev/null 2>&1
	systemctl restart r3v3rs3-agent
	if [ -z "$token" ]; then
		sleep 2
		systemctl is-active --quiet r3v3rs3-agent || agent_failed "the agent service did not start"
		return
	fi
	if ! wait_for_enrollment; then
		systemctl stop r3v3rs3-agent
		rm -f "$AGENT_TOKEN_FILE"
		restore_identity
		# An agent of an earlier enrollment keeps running with its old identity.
		if is_enrolled; then
			systemctl start r3v3rs3-agent
		fi
		agent_failed "the agent did not enroll at $master within 30 seconds"
	fi
	# A token works only once, so it has no use after the enrollment.
	rm -f "$AGENT_TOKEN_FILE"
	drop_old_identity
}

main_agent() {
	check_docker
	local target
	target=$(detect_target)
	choose_master
	choose_token
	install_binary "$target"

	install -d -m 0700 "$AGENT_DATA_DIR"
	if [ -n "$token" ]; then
		set_aside_identity
		write_token_file
	fi
	write_agent_unit
	start_agent

	cat <<EOF

The r3v3rs3 $installed_tag agent is running and connected to $master.

  Binary:   $BIN
  Data:     $AGENT_DATA_DIR
  Logs:     journalctl -u r3v3rs3-agent
  Service:  systemctl status|restart|stop r3v3rs3-agent

Run the script again with --agent to upgrade. The Targets page of the master shows the state of
the target.
EOF
}

main() {
	parse_args "$@"
	check_system
	if [ "$agent" = true ]; then
		main_agent
		return
	fi
	local target
	target=$(detect_target)
	choose_webui
	install_binary "$target"

	install -d -m 0700 "$CONFIG_DIR"
	install -d -m 0750 "$LOG_DIR"
	create_admin

	write_unit
	systemctl daemon-reload
	systemctl enable r3v3rs3 >/dev/null 2>&1
	systemctl restart r3v3rs3
	open_firewall
	wait_for_webui
	create_admin

	cat <<EOF

r3v3rs3 $installed_tag is running.

  WebUI:    http://$webui/
  Binary:   $BIN
  Config:   $CONFIG_DIR
  Logs:     $LOG_DIR and journalctl -u r3v3rs3
  Service:  systemctl status|restart|stop r3v3rs3
  Add user: $BIN add-user --config-dir $CONFIG_DIR <name>
EOF
	if [ "$account_created" = true ]; then
		cat <<EOF

  Admin:    admin
  Password: $password

  The script printed the password only here. Store it in a password manager now.
EOF
	fi
	cat <<EOF

Run the script again to upgrade.
EOF
}

installed_tag=""
account_created=false
main "$@"
