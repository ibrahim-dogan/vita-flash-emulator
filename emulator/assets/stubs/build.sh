#!/usr/bin/env bash
# Rebuilds portal_api.swf from the .as sources with Ruffle's asc.jar.
# Needs java and a built ruffle_core (for playerglobal.swf). The output is
# committed, so normal builds don't need any of this.
set -euo pipefail
cd "$(dirname "$0")"

ASC=$(ls ~/.cargo/git/checkouts/ruffle-*/*/core/build_playerglobal/asc.jar | head -1)
PG=$(ls -t ../../target/*/build/ruffle_core/*/out/playerglobal.swf ../../target/*/*/build/ruffle_core/*/out/playerglobal.swf 2>/dev/null | head -1)
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

# asc.jar imports definitions from a bare .abc: pull the DoABC tag out of playerglobal.swf.
python3 - "$PG" "$TMP/playerglobal.abc" <<'EOF'
import struct, sys, zlib
d = open(sys.argv[1], 'rb').read()
body = zlib.decompress(d[8:]) if d[:3] == b'CWS' else d[8:]
p = ((5 + 4 * (body[0] >> 3) + 7) // 8) + 4
while p < len(body):
    h = struct.unpack('<H', body[p:p + 2])[0]; p += 2
    code, ln = h >> 6, h & 0x3f
    if ln == 0x3f:
        ln = struct.unpack('<I', body[p:p + 4])[0]; p += 4
    if code == 82:  # DoABC2: flags u32, name string, abc
        t = body[p:p + ln]; q = t.index(0, 4)
        open(sys.argv[2], 'wb').write(t[q + 1:])
        break
    p += ln
EOF

cp AnyStub.as PortalApi.as "$TMP/"
java -cp "$ASC" macromedia.asc.embedding.Main -AS3 -optimize \
    -import "$TMP/playerglobal.abc" \
    -in "$TMP/AnyStub.as" \
    -swf PortalApi,1,1 "$TMP/PortalApi.as"
cp "$TMP/PortalApi.swf" portal_api.swf
ls -la portal_api.swf
