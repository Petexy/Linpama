#!/bin/bash
# Plays the same steps on the native widget (harness.rs) and the original
# Python widget (python_reference.py) in a private headless KWin and compares
# every pair: PNGs pixel by pixel, the lines both print, the commands both ran
# (and the web addresses both read) and the files they left.
#
#   cargo build --lib --examples
#   examples/compare.sh                 # ONLY=REGEX limits the scenarios
#
# Nothing touches the real session, system or network: a private D-Bus
# session, HOME, TMPDIR and the XDG directories under .test-env, rebuilt before
# every run; fake pacman, paru, git, flatpak, curl and sudo wrapper first on
# PATH, answering from the fixtures below; the Python widget's urllib reads
# the same fixtures. The host and the Python stand-in accept only "secret".
#
# With LXB_LIBRARY_DIR naming a folder with a liblxb_app.so that has
# lxb_app_shot_after and lets a page take text (lxb_page_taking_text), the
# LineXinBar page is played too (examples/lxb_run.rs, with what is typed
# scripted): a fake pkexec runs the fake pacman, and what the page ran is
# checked.
set -u
examples=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$examples/.." && pwd)
env_dir=$repo/.test-env
out=$env_dir/compare
fixtures=$env_dir/fixtures
socket=${LINEXIN_TEST_SOCKET:-linexin-test-linpama}
# Where cargo built this checkout (CARGO_TARGET_DIR and cargo's settings
# count), asked before the private session, which has no cargo on PATH.
target=${LINEXIN_COMPARE_TARGET:-$(cd "$repo" && cargo metadata --no-deps --format-version 1 |
    python3 -c 'import json, sys; print(json.load(sys.stdin)["target_directory"])')/debug}
# The original Python widget: installed, or the copy PYTHON_WIDGET names.
python_widget=${PYTHON_WIDGET:-/usr/share/linexin/widgets/y-package_manager.py}

if [ -z "${LINEXIN_LINPAMA_SESSION:-}" ]; then
    harness=$target/examples/harness
    library=$target/libpackage_manager.so
    [ -x "$harness" ] && [ -f "$library" ] || { echo "run: cargo build --lib --examples" >&2; exit 2; }
    [ -f "$python_widget" ] || {
        echo "No original Python widget at $python_widget: install it, or name its file in PYTHON_WIDGET" >&2
        exit 2
    }
    rm -rf "$env_dir"
    mkdir -p "$out/png" "$out/logs" "$env_dir/bin" "$env_dir/widgets" "$fixtures/http" "$fixtures/aur/hello-aur"
    cp "$library" "$env_dir/widgets/y-package_manager.so"

    # pacman's own listings.
    cat >"$fixtures/installed.txt" <<'EOF'
local/bash 5.3.3-2 (base)
    The GNU Bourne Again shell
local/firefox 143.0.4-1
    Fast, Private & Safe Web Browser
local/linux 6.17.1.arch1-1
    The Linux kernel and modules
local/nano 8.6-1
    Pico editor clone with enhancements
local/vlc 3.0.21-21
    Free and open source cross-platform multimedia player and framework
EOF
    cat >"$fixtures/search-fire.txt" <<'EOF'
extra/firefox 143.0.4-1 [installed]
    Fast, Private & Safe Web Browser
extra/firefox-developer-edition 144.0b9-1
    Fast, Private & Safe Web Browser (Developer Edition)
extra/firefox-i18n-pl 143.0.4-1
    Polish language pack for Firefox
extra/firejail 0.9.76-1
    Linux namespaces sandbox program
extra/bonfire 1.0-1
    Something that mentions fire
EOF
    printf 'bash\nfirefox\nlinux\nnano\nvlc\nhello-aur-git\n' >"$fixtures/qq.txt"
    printf 'firefox 143.0.4-1\nbash 5.3.3-2\n' >"$fixtures/q.txt"
    printf 'core bash 5.3.3-2 [installed]\nextra firefox 143.0.4-1 [installed]\nextra-testing firefox 144.0-1\n' >"$fixtures/sl.txt"
    printf 'org.mozilla.firefox\norg.videolan.VLC\norg.gnome.TextEditor\n' >"$fixtures/flatpak.txt"
    # The network, by address (as the fake curl and python_reference.py name it).
    http() { printf '%s' "$1" | sed 's/[^A-Za-z0-9._-]/_/g'; }
    cat >"$fixtures/http/$(http 'https://aur.archlinux.org/rpc/?v=5&type=search&arg=fire')" <<'EOF'
{"resultcount":3,"results":[{"Name":"firefox","Version":"143.0.4-1","Description":"duplicate of a repository package"},{"Name":"fire-aur","Version":"2.1-3","Description":"An AUR package about fire"},{"Name":"campfire-git","Version":"r12.abc-1","Description":null}],"type":"search","version":5}
EOF
    cat >"$fixtures/http/$(http 'https://aur.archlinux.org/rpc/?v=5&type=search&arg=hello')" <<'EOF'
{"resultcount":1,"results":[{"Name":"hello-aur","Version":"1.0-1","Description":"Says hello from the AUR"}],"type":"search","version":5}
EOF
    cat >"$fixtures/http/$(http 'https://wiki.archlinux.org/api.php?action=parse&prop=text&format=json&redirects=1&page=firefox')" <<'EOF'
{"parse":{"title":"Firefox","pageid":1,"text":{"*":"<p><b>Firefox</b> is a popular open source graphical web browser from Mozilla.</p><h2>Installation</h2><p>Install the <a href=\"/title/Firefox\">firefox</a> package.</p><pre>pacman -S firefox</pre>"}}}
EOF
    cat >"$fixtures/http/$(http 'https://archive.archlinux.org/packages/f/firefox/')" <<'EOF'
<html><body><pre>
<a href="firefox-142.0-1-x86_64.pkg.tar.zst">firefox-142.0-1-x86_64.pkg.tar.zst</a>
<a href="firefox-142.0-1-x86_64.pkg.tar.zst.sig">firefox-142.0-1-x86_64.pkg.tar.zst.sig</a>
<a href="firefox-143.0.4-1-x86_64.pkg.tar.zst">firefox-143.0.4-1-x86_64.pkg.tar.zst</a>
<a href="firefox-1%3A99.0-2-x86_64.pkg.tar.xz">firefox-1:99.0-2-x86_64.pkg.tar.xz</a>
</pre></body></html>
EOF
    printf 'fake package\n' >"$fixtures/http/$(http 'https://archive.archlinux.org/packages/f/firefox/firefox-142.0-1-x86_64.pkg.tar.zst')"
    printf 'pkgname=hello-aur\npkgver=1.0\npkgrel=1\narch=(any)\npackage() { :; }\n' >"$fixtures/aur/hello-aur/PKGBUILD"

    cat >"$env_dir/bin/fake-wrapper" <<'EOF'
#!/bin/sh
echo "wrapper $*" >>"$FAKE_LOG"
exec "$@"
EOF
    cat >"$env_dir/bin/pacman" <<'EOF'
#!/bin/sh
echo "pacman $*" >>"$FAKE_LOG"
f=$FAKE_FIXTURES
case "$1" in
    -Qs) cat "$f/installed.txt"; exit 0 ;;
    -Ss) [ -f "$f/search-$2.txt" ] && { cat "$f/search-$2.txt"; exit 0; }; exit 1 ;;
    -Qq) cat "$f/qq.txt"; exit 0 ;;
    -Q) grep "^$2 " "$f/q.txt" && exit 0; echo "error: package '$2' was not found" >&2; exit 1 ;;
    -Sl) cat "$f/sl.txt"; exit 0 ;;
    -Qdtq) [ -n "${FAKE_ORPHANS:-}" ] || exit 1; printf '%s\n' $FAKE_ORPHANS; exit 0 ;;
esac
printf 'resolving dependencies...\nlooking for conflicting packages...\n'
printf '(1/1) checking keys in keyring  [####] 50%%\r(1/1) checking keys in keyring  [########] 100%%\n'
[ -n "${FAKE_PACMAN_SLEEP:-}" ] && sleep "$FAKE_PACMAN_SLEEP"
status=${FAKE_PACMAN_STATUS:-0}
[ "$status" != 0 ] && echo "error: failed to commit transaction (conflicting files)" >&2
exit "$status"
EOF
    cat >"$env_dir/bin/paru" <<'EOF'
#!/bin/sh
echo "paru $* PACMAN_AUTH=${PACMAN_AUTH##*/} SUDO_ASKPASS=$SUDO_ASKPASS LC_ALL=$LC_ALL" >>"$FAKE_LOG"
printf ':: Resolving dependencies...\n==> Making package: hello-aur 1.0-1\n'
exit "${FAKE_PARU_STATUS:-0}"
EOF
    cat >"$env_dir/bin/git" <<'EOF'
#!/bin/sh
echo "git $1 $2" >>"$FAKE_LOG"
if [ "$1" = clone ]; then
    name=${2##*/}; name=${name%.git}
    if [ -d "$FAKE_FIXTURES/aur/$name" ]; then
        cp -r "$FAKE_FIXTURES/aur/$name/." "$3/"
        echo "Cloning into '$3'..."
        exit 0
    fi
    echo "fatal: repository not found" >&2
    exit 128
fi
exit 0
EOF
    cat >"$env_dir/bin/flatpak" <<'EOF'
#!/bin/sh
[ "$1" = remote-ls ] && cat "$FAKE_FIXTURES/flatpak.txt"
exit 0
EOF
    # curl [options] URL: the fixture named after the URL, or a 404.
    cat >"$env_dir/bin/curl" <<'EOF'
#!/bin/sh
output=; write=; url=
while [ $# -gt 0 ]; do
    case "$1" in
        --output) output=$2; shift ;;
        --write-out) write=$2; shift ;;
        --connect-timeout|--speed-time|--speed-limit) shift ;;
        -*) ;;
        *) url=$1 ;;
    esac
    shift
done
echo "http $url" >>"$FAKE_LOG"
file=$FAKE_HTTP/$(printf '%s' "$url" | sed 's/[^A-Za-z0-9._-]/_/g')
if [ ! -f "$file" ]; then
    printf '%b' "$(printf '%s' "$write" | sed 's/%{http_code}/404/')"
    echo "curl: (22) The requested URL returned error: 404" >&2
    exit 22
fi
if [ -n "$output" ]; then cp "$file" "$output"; else cat "$file"; fi
printf '%b' "$(printf '%s' "$write" | sed 's/%{http_code}/200/')"
EOF
    cat >"$env_dir/bin/fake" <<'EOF'
#!/bin/sh
echo "${0##*/} $*" >>"$FAKE_LOG"
exit 1
EOF
    chmod 755 "$env_dir/bin/"*
    for name in sudo pkexec xdg-open; do ln -s fake "$env_dir/bin/$name"; done
    # The LineXinBar page asks with pkexec, and runs what it was asked.
    mkdir -p "$env_dir/bin-lxb"
    cat >"$env_dir/bin-lxb/pkexec" <<'EOF'
#!/bin/sh
echo "pkexec $*" >>"$FAKE_LOG"
exec "$@"
EOF
    cat >"$env_dir/bin-lxb/distribumpy" <<'EOF'
#!/bin/sh
echo "distribumpy $*" >>"$FAKE_LOG"
EOF
    chmod 755 "$env_dir/bin-lxb/"*
    if [ -n "${LXB_LIBRARY_DIR:-}" ]; then
        mkdir -p "$env_dir/lxb"
        cp "$LXB_LIBRARY_DIR"/liblxb_app.so "$LXB_LIBRARY_DIR"/liblxb_toolkit.so "$env_dir/lxb/" || exit 2
        LXB_LIBRARY_DIR=$env_dir/lxb
    fi
    cat >"$env_dir/bus.conf" <<'EOF'
<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:tmpdir=/tmp</listen>
  <policy context="default"><allow send_destination="*"/><allow receive_sender="*"/><allow own="*"/></policy>
</busconfig>
EOF
    "$harness" --print-css >"$env_dir/center-style.css" || exit 2
    exec env -i LINEXIN_COMPARE_TARGET="$target" PYTHON_WIDGET="$python_widget" \
        LINEXIN_CENTER_CSS="$env_dir/center-style.css" PATH="$env_dir/bin:/usr/bin" HOME="$env_dir/home" TMPDIR="$env_dir/tmp" \
        USER="$(id -un)" \
        XDG_CONFIG_HOME="$env_dir/home/.config" XDG_CACHE_HOME="$env_dir/home/.cache" \
        XDG_DATA_HOME="$env_dir/home/.local/share" XDG_RUNTIME_DIR="$XDG_RUNTIME_DIR" \
        LANG=en_US.UTF-8 ADW_DISABLE_PORTAL=1 GDK_DEBUG=no-portals GTK_A11Y=none \
        FAKE_WRAPPER="$env_dir/bin/fake-wrapper" FAKE_PASSWORD=secret \
        FAKE_FIXTURES="$fixtures" FAKE_HTTP="$fixtures/http" \
        LINEXIN_LINPAMA_SESSION=1 ONLY="${ONLY:-}" LXB_LIBRARY_DIR="${LXB_LIBRARY_DIR:-}" \
        dbus-run-session --config-file="$env_dir/bus.conf" -- bash -c '
        kwin_wayland --virtual --no-lockscreen --width 1600 --height 1000 --socket "$1" \
            >"$2/logs/kwin.log" 2>&1 &
        kwin=$!
        for _ in $(seq 100); do [ -S "$XDG_RUNTIME_DIR/$1" ] && break; sleep 0.1; done
        WAYLAND_DISPLAY=$1 GDK_BACKEND=wayland "$3"
        status=$?
        kill $kwin; wait $kwin 2>/dev/null
        exit $status' session "$socket" "$out" "$0"
fi

harness=$target/examples/harness
library=$env_dir/widgets/y-package_manager.so
total=0
differing=0

# fixture WARN: a fresh HOME and TMPDIR; with WARN=no the stability warning
# was turned off before.
fixture() {
    rm -rf "$HOME" "$TMPDIR"
    mkdir -p "$HOME/.config" "$HOME/.cache" "$HOME/.local/share" "$TMPDIR"
    if [ "$1" = no ]; then
        mkdir -p "$HOME/.config/linpama"
        printf '{"show_warning": false}' >"$HOME/.config/linpama/config.json"
    fi
}

# What differs between two runs by chance: mkdtemp's random names.
normalize() {
    sed -E 's/(linpama_aur_|linpama_)[A-Za-z0-9_]{6,8}/\1RANDOM/g' "$1"
}

# pair TAG WARN ENV OPTIONS... STEPS...: both sides with the same fixture and
# environment (ENV: VAR=value[;VAR=value]... or "-"); the PNG names (@) are
# rewritten per side, into png/TAG/.
pair() {
    local tag=$1 warn=$2 vars=$3
    shift 3
    [ -n "$ONLY" ] && ! [[ $tag =~ $ONLY ]] && return
    local side extra=()
    mkdir -p "$out/png/$tag"
    [ "$vars" != - ] && IFS=';' read -r -a extra <<<"$vars"
    for side in rust py; do
        fixture "$warn"
        local args=() arg
        for arg in "$@"; do args+=("${arg//@/$out/png/$tag/$side}"); done
        local base=$out/logs/$side-$tag
        : >"$base.commands"
        : >"$base.locks"
        (
            cd "$HOME" || exit 1
            if [ $side = rust ]; then
                env "${extra[@]}" FAKE_LOG="$base.commands" FAKE_LOCK_LOG="$base.locks" \
                    timeout 120 "$harness" --library "$library" "${args[@]}"
            else
                env "${extra[@]}" FAKE_LOG="$base.commands" FAKE_LOCK_LOG="$base.locks" \
                    timeout 120 python3 -B "$examples/python_reference.py" "${args[@]}"
            fi
        ) >"$base.out" 2>"$base.err"
        echo "exit $?" >>"$base.out"
        sort -o "$base.locks" "$base.locks"
        # The order of a search's own commands and of the wiki's requests
        # depends on threads; what was run is compared, sorted.
        sort -o "$base.commands" "$base.commands"
    done
    local differences=0 png kind
    if ! diff -q <(normalize "$out/logs/rust-$tag.out" | grep -v '^rendered' | grep -v '^widgetname') \
        <(normalize "$out/logs/py-$tag.out" | grep -v '^rendered' | grep -v '^widgetname') >/dev/null; then
        echo "$tag: printed lines differ"
        diff <(normalize "$out/logs/rust-$tag.out" | grep -v '^widgetname') \
            <(normalize "$out/logs/py-$tag.out" | grep -v '^widgetname') | head -14
        differences=1
    fi
    for kind in commands locks; do
        if ! diff -q <(normalize "$out/logs/rust-$tag.$kind") <(normalize "$out/logs/py-$tag.$kind") >/dev/null; then
            echo "$tag: $kind differ"
            diff <(normalize "$out/logs/rust-$tag.$kind") <(normalize "$out/logs/py-$tag.$kind") | head -10
            differences=1
        fi
    done
    local pairs=0
    for png in "$out/png/$tag"/rust*.png; do
        [ -e "$png" ] || continue
        pairs=$((pairs + 1))
        local other=$out/png/$tag/py${png##*/rust}
        if ! python3 - "$png" "$other" <<'EOF'
import sys
from PIL import Image, ImageChops
a, b = (Image.open(p).convert("RGBA") for p in sys.argv[1:3])
if a.size != b.size:
    print(f"{sys.argv[1]}: size {a.size} vs {b.size}")
    sys.exit(1)
box = ImageChops.difference(a, b).getbbox()
if box:
    print(f"{sys.argv[1]}: differs in {box}")
    sys.exit(1)
EOF
        then
            differences=1
        fi
    done
    total=$((total + 1))
    if [ $differences = 0 ]; then
        echo "$tag: $pairs image pair(s), identical"
    else
        differing=$((differing + 1))
    fi
}

unlock=answer=unlock=secret
for lang in en_US pl_PL; do
    pair "$lang-warning" yes - --lang $lang print shot=@.png
    pair "$lang-installed" no - --lang $lang wait-results print shot=@.png
done
pair "dark-warning" yes - --dark shot=@.png
pair "dark-installed" no - --dark wait-results shot=@.png
pair "warning-gnome" yes XDG_CURRENT_DESKTOP=GNOME print shot=@.png
pair "continue" yes - "check=Do not show this warning again" "click=I Understand, Continue" \
    settle wait-results print files shot=@.png
pair "compact" no - --width 800 wait-results print shot=@.png sidebar settle print shot=@-panel.png
pair "search" no - type=fire sleep=1 wait-results settle print shot=@.png
pair "search-aur-pl" no - --lang pl_PL "check=Szukaj w AUR" type=fire sleep=1 wait-results settle \
    print shot=@.png
pair "no-results" no - type=zzzz sleep=1 wait-results settle print shot=@.png
pair "detail" no - wait-results row=firefox settle sleep=2 print shot=@.png
pair "detail-no-wiki" no - wait-results row=nano settle sleep=1 print shot=@.png
pair "detail-pl" no - --lang pl_PL wait-results row=vlc settle sleep=1 print shot=@.png
pair "install" no FAKE_PACMAN_SLEEP=2 type=fire sleep=1 wait-results settle \
    row-action=firejail "$unlock" settle print shot=@-progress.png wait-done settle print \
    shot=@.png "click=View Log" settle print
pair "remove-fails" no FAKE_PACMAN_STATUS=1 wait-results row-action=vlc "$unlock" wait-done \
    settle print shot=@.png
pair "wrong-password" no - wait-results row-action=vlc answer=unlock=nope settle print \
    dialog-shot=@-dialog.png
pair "versions" no - wait-results row=firefox settle "click=Change Version" settle print \
    dialog-shot=@-dialog.png select=142.0-1 answer=install "$unlock" wait-done settle print
pair "refresh-fails" no FAKE_PACMAN_STATUS=1 wait-results "activate=Refresh Repositories" \
    "$unlock" settle print dialog-shot=@-dialog.png
pair "clear-cache" no - wait-results "activate=Clear Package Cache" "$unlock" wait-done settle print
pair "orphans-none" no - wait-results "activate=Remove Orphans" "$unlock" settle print
pair "orphans" no "FAKE_ORPHANS=libfoo python-bar" wait-results "activate=Remove Orphans" "$unlock" \
    settle print dialog-shot=@-dialog.png answer=remove wait-done settle print
pair "db-lock" no - wait-results "activate=Remove DB Lock" "$unlock" wait-done settle print
pair "aur-install" no - "check=Search AUR" type=hello sleep=1 wait-results settle \
    row-action=hello-aur wait=pkgbuild_view settle print shot=@-pkgbuild.png \
    "click=Proceed to Build" "$unlock" wait-done settle print
pair "aur-cancel" no - "check=Search AUR" type=hello sleep=1 wait-results settle \
    row-action=hello-aur wait=pkgbuild_view settle click=Cancel settle print
pair "aur-missing" no - "check=Search AUR" type=fire sleep=1 wait-results settle \
    row-action=fire-aur settle sleep=1 print

# lxb TAG WARN ENV FRAMES STEPS... -- EXPECTED...: the LineXinBar page alone
# for FRAMES frames (sixty a second), the FRAME:ACTION steps pressed, what is
# typed in ENV's LINEXIN_LXB_TYPED; EXPECTED are KIND~TEXT that must be found:
# commands (what ran), out (what it printed), config (the warning's
# config.json), or nocommands~TEXT that must not have run.
lxb() {
    local tag=$1 warn=$2 vars=$3 frames=$4
    shift 4
    [ -n "$ONLY" ] && ! [[ $tag =~ $ONLY ]] && return
    if [ -z "${LXB_LIBRARY_DIR:-}" ]; then
        echo "$tag: skipped (no LXB_LIBRARY_DIR)"
        return
    fi
    local steps=() env=()
    while [ $# -gt 0 ] && [ "$1" != -- ]; do steps+=("$1"); shift; done
    shift
    [ "$vars" != - ] && IFS=';' read -r -a env <<<"$vars"
    fixture "$warn"
    local base=$out/logs/lxb-$tag
    : >"$base.commands"
    mkdir -p "$out/png/$tag"
    (
        cd "$HOME" || exit 1
        env "${env[@]}" PATH="$env_dir/bin-lxb:$PATH" FAKE_LOG="$base.commands" \
            LD_LIBRARY_PATH="$LXB_LIBRARY_DIR" \
            timeout 120 "$target/examples/lxb_run" --library "$library" \
            --out "$out/png/$tag/lxb.png" --frames "$frames" "${steps[@]}"
    ) >"$base.out" 2>"$base.err"
    echo "exit $?" >>"$base.out"
    cat "$HOME/.config/linpama/config.json" >"$base.config" 2>/dev/null || : >"$base.config"
    total=$((total + 1))
    local missing=0 what kind text
    grep -q '^exit 0$' "$base.out" || { echo "$tag: the page failed: $(tail -1 "$base.err")"; missing=1; }
    for what in "$@"; do
        kind=${what%%~*}
        text=${what#*~}
        if [ "$kind" = nocommands ]; then
            if grep -qF -- "$text" "$base.commands"; then
                echo "$tag: ran $text"
                missing=1
            fi
        elif ! grep -qF -- "$text" "$base.$kind"; then
            echo "$tag: $kind lacks $text"
            missing=1
        fi
    done
    if [ $missing = 0 ]; then
        echo "$tag: the LineXinBar page did what it should"
    else
        differing=$((differing + 1))
    fi
}

# Every other frame from FIRST, COUNT times: "FRAME:ACTION" steps.
steps() {
    local first=$1 count=$2 action=$3 i
    for ((i = 0; i < count; i++)); do printf '%s:%s ' $((first + i * 2)) "$action"; done
}

# The page is wide enough for the actions' panel: the search page's controls
# are the field (0), Search AUR (1), then the installed packages (bash,
# firefox, linux, nano, vlc) or the results; Right goes into the panel.
lxb "lxb-search" no "LINEXIN_LXB_TYPED=0.3:fire" 150 -- \
    "commands~pacman -Qs" "commands~pacman -Ss fire" "nocommands~https://aur.archlinux.org"
lxb "lxb-rubbed" no 'LINEXIN_LXB_TYPED=0.3:firx|0.4:\bejail' 150 -- "commands~pacman -Ss firejail"
lxb "lxb-aur" no "LINEXIN_LXB_TYPED=0.9:hello" 160 30:down 32:accept 40:up -- \
    "commands~http https://aur.archlinux.org/rpc/?v=5&type=search&arg=hello"
lxb "lxb-aur-build" no "LINEXIN_LXB_TYPED=0.9:hello" 400 30:down 32:accept 40:up \
    120:down 122:down 124:accept 160:accept 230:accept -- \
    "commands~git clone https://aur.archlinux.org/hello-aur.git" \
    "commands~paru -S --noconfirm --sudo pkexec hello-aur PACMAN_AUTH=pkexec"
# shellcheck disable=SC2046
lxb "lxb-remove" no - 200 $(steps 20 6 down) 50:accept 90:accept -- \
    "commands~pkexec pacman -Rns --noconfirm vlc" "commands~pacman -Rns --noconfirm vlc"
# shellcheck disable=SC2046
lxb "lxb-install" no "LINEXIN_LXB_TYPED=0.3:fire" 260 $(steps 60 5 down) 80:accept 120:accept -- \
    "commands~pkexec pacman -S --noconfirm --needed firejail" \
    "commands~http https://wiki.archlinux.org/api.php?action=parse&prop=text&format=json&redirects=1&page=firejail"
# shellcheck disable=SC2046
lxb "lxb-details" no - 160 $(steps 20 3 down) 40:accept -- \
    "commands~http https://wiki.archlinux.org/api.php?action=parse&prop=text&format=json&redirects=1&page=firefox"
# shellcheck disable=SC2046
lxb "lxb-versions-menu" no - 200 $(steps 20 3 down) 40:accept 80:down 82:accept -- \
    "commands~pacman -Sl" "commands~http https://archive.archlinux.org/packages/f/firefox/"
# The menu opens on the installed version, the archive's older one under it
# (the real package cache adds newer rows above).
# shellcheck disable=SC2046
lxb "lxb-versions" no - 420 $(steps 20 3 down) 40:accept 80:down 82:accept 300:down 320:accept -- \
    "commands~http https://archive.archlinux.org/packages/f/firefox/firefox-142.0-1-x86_64.pkg.tar.zst" \
    "commands~pkexec pacman -U --noconfirm"
lxb "lxb-refresh" no - 120 20:right 24:accept -- "commands~pkexec pacman -Sy"
lxb "lxb-clear-cache" no - 120 20:right 22:down 26:accept -- \
    "commands~pkexec pacman -Scc --noconfirm" "commands~paru -Scc --noconfirm"
# shellcheck disable=SC2046
lxb "lxb-orphans" no "FAKE_ORPHANS=libfoo python-bar" 200 20:right 22:down 24:down 30:accept \
    80:right 84:accept -- "commands~pkexec pacman -Rns --noconfirm libfoo python-bar"
lxb "lxb-warning" yes "XDG_CURRENT_DESKTOP=LineXinBar" 120 20:accept 30:down 32:down 34:accept \
    40:up 44:accept -- "commands~distribumpy" 'config~{"show_warning": false}' "commands~pacman -Qs"

echo "$total scenarios, $differing with differences"
[ $differing = 0 ]
