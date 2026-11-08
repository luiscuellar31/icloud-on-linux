#!/usr/bin/env bash
set +x
set -euo pipefail
umask 077

session_mode="${1:-shell}"
case "$session_mode" in
    shell|mount|check|auth-init|auth-inspect|auth-create|auth-answer|auth-password|auth-probe) ;;
    *) printf 'Invalid session mode.\n' >&2; exit 1 ;;
esac

: "${RCLONE_CONFIG:?The rclone configuration path is missing}"
trap 'unset RCLONE_CONFIG_PASS confirmation' EXIT

case "$session_mode" in
    auth-init)
        [[ ! -e "$RCLONE_CONFIG" && -n "${RCLONE_CONFIG_PASS:-}" ]] || exit 20
        exec rclone --ask-password=false config encryption set --password-command 'printenv RCLONE_CONFIG_PASS'
        ;;
    auth-*)
        [[ -f "$RCLONE_CONFIG" && -O "$RCLONE_CONFIG" && ! -L "$RCLONE_CONFIG" ]] || exit 20
        grep -q '^RCLONE_ENCRYPT_V0:' "$RCLONE_CONFIG" || exit 21
        case "$session_mode" in
            # Only the remote's type, service, and account leave rclone; its password and tokens stay there.
            auth-inspect)
                rclone --ask-password=false config show icloud | { grep -E '^(\[icloud\]$|(type|service|apple_id) = )' || true; }
                exit
                ;;
            auth-create) exec rclone --ask-password=false config create icloud iclouddrive --all --non-interactive ;;
            auth-answer) exec rclone --ask-password=false config update icloud --all --non-interactive --continue ;;
            auth-password)
                # Continued answers are saved verbatim, but the backend reads its password obscured.
                RCLONE_RESULT="$(printenv RCLONE_RESULT | rclone --ask-password=false obscure -)"
                exec rclone --ask-password=false config update icloud --all --non-interactive --continue
                ;;
            auth-probe) exec rclone --ask-password=false --contimeout 15s --timeout 45s --retries 1 --low-level-retries 1 lsjson --stat icloud: ;;
        esac
        ;;
esac

if [[ "$session_mode" == check ]]; then
    [[ -f "$RCLONE_CONFIG" && -O "$RCLONE_CONFIG" && ! -L "$RCLONE_CONFIG" ]] || exit 20
    grep -q '^RCLONE_ENCRYPT_V0:' "$RCLONE_CONFIG" || exit 21
    IFS= read -r -s RCLONE_CONFIG_PASS || exit 22
    [[ -n "$RCLONE_CONFIG_PASS" ]] || exit 22
    export RCLONE_CONFIG_PASS
    remotes="$(rclone --ask-password=false listremotes 2>/dev/null)" || exit 22
    case $'\n'"$remotes"$'\n' in
        *$'\nicloud:\n'*) exit 0 ;;
        *) exit 23 ;;
    esac
fi

printf 'iCloud Drive - read-only lab\n\n'
printf 'The local key protects your saved session and is separate from your Apple password.\n'
IFS= read -r -s -p 'Local key: ' RCLONE_CONFIG_PASS
printf '\n'
if [[ -z "$RCLONE_CONFIG_PASS" ]]; then
    printf 'The local key cannot be empty.\n' >&2
    exit 1
fi

if [[ ! -s "$RCLONE_CONFIG" ]] || ! grep -q '^RCLONE_ENCRYPT_V0:' "$RCLONE_CONFIG"; then
    IFS= read -r -s -p 'Confirm the new local key: ' confirmation
    printf '\n'
    if [[ "$RCLONE_CONFIG_PASS" != "$confirmation" ]]; then
        printf 'The keys do not match. Open the lab again.\n' >&2
        exit 1
    fi
    unset confirmation
fi

export RCLONE_CONFIG_PASS
rclone --ask-password=false config encryption set \
    --password-command 'printenv RCLONE_CONFIG_PASS' >/dev/null
rclone --ask-password=false config encryption check

remotes="$(rclone --ask-password=false listremotes)"
case $'\n'"$remotes"$'\n' in
    *$'\nicloud:\n'*) ;;
    *)
        printf '\nConnection prepared: icloud. Service: Drive (the default).\n'
        printf 'Enter your Apple account and password. Complete 2FA when prompted.\n'
        printf 'You can keep the defaults for advanced options.\n\n'
        rclone --ask-password=false config create icloud iclouddrive --all --no-output
        ;;
esac

printf '\nChecking read access to iCloud Drive...\n'
if ! rclone --ask-password=false lsf icloud:; then
    printf '\nCould not read iCloud Drive. Keep the error above for diagnosis.\n' >&2
    exit 1
fi

if [[ "$session_mode" == mount ]]; then
    mkdir -p "$HOME/iCloud"
    rclone --ask-password=false mount icloud: "$HOME/iCloud" \
        --read-only --vfs-cache-mode full --vfs-cache-max-size 1G \
        --daemon --daemon-wait 30s
    unset RCLONE_CONFIG_PASS
    printf '\niCloud is available in the iCloud folder.\n'
    gio open "$HOME/iCloud"
else
    printf '\nAccess verified. Follow the README to explore or download a file.\n'
    printf 'Type exit to close the lab.\n\n'
    exec bash --noprofile --norc
fi
