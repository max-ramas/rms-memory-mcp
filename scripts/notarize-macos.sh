#!/usr/bin/env bash
# Sends a signed .app, .zip or .dmg to Apple's notary service, waits, and staples the ticket.
#
#   scripts/notarize.sh <path>
#
# Credentials, in this order:
#   NOTARY_PROFILE                              a keychain profile made once with `xcrun notarytool store-credentials`
#   APPLE_ID + APPLE_APP_PASSWORD + APPLE_TEAM_ID   (CI secrets; the password is an app-specific password)
# Nothing is printed except the notary log; credentials never reach the output.
set -euo pipefail
TARGET="${1:?usage: notarize.sh <path to .app, .zip or .dmg>}"
[[ -e "$TARGET" ]] || { echo "no such file: $TARGET" >&2; exit 2; }

if [[ -n "${NOTARY_PROFILE:-}" ]]; then
    AUTH=(--keychain-profile "$NOTARY_PROFILE")
elif [[ -n "${APPLE_ID:-}" && -n "${APPLE_APP_PASSWORD:-}" && -n "${APPLE_TEAM_ID:-}" ]]; then
    AUTH=(--apple-id "$APPLE_ID" --password "$APPLE_APP_PASSWORD" --team-id "$APPLE_TEAM_ID")
else
    echo "notarization credentials are not set (NOTARY_PROFILE, or APPLE_ID + APPLE_APP_PASSWORD + APPLE_TEAM_ID)" >&2
    exit 3
fi

SUBMIT="$TARGET"
TMP=""
if [[ -d "$TARGET" ]]; then
    TMP="$(mktemp -d)"
    trap 'rm -rf "$TMP"' EXIT
    SUBMIT="$TMP/$(basename "$TARGET").zip"
    ditto -c -k --keepParent "$TARGET" "$SUBMIT"
fi

OUT="$(xcrun notarytool submit "$SUBMIT" "${AUTH[@]}" --wait --timeout 150m --output-format json)"
STATUS="$(printf '%s' "$OUT" | plutil -extract status raw -o - - 2>/dev/null || true)"
ID="$(printf '%s' "$OUT" | plutil -extract id raw -o - - 2>/dev/null || true)"
echo "notarization $ID: ${STATUS:-unknown}"
if [[ "$STATUS" != "Accepted" ]]; then
    [[ -n "$ID" ]] && xcrun notarytool log "$ID" "${AUTH[@]}" >&2 || true
    exit 4
fi

# A zip cannot carry a ticket; only the app inside it, or a dmg, can be stapled.
case "$TARGET" in
    *.zip) echo "zip notarized (staple the .app before zipping it)" ;;
    *) xcrun stapler staple "$TARGET" && xcrun stapler validate "$TARGET" ;;
esac
