#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
fixture=$(mktemp -d /tmp/own-player-ui.XXXXXX)
trap 'rm -rf "$fixture"' EXIT
export GDK_BACKEND=x11
export XDG_DATA_HOME="$fixture/data"
export XDG_CACHE_HOME="$fixture/cache"
export OWN_PLAYER_SCREENSHOTS="$(pwd)/artifacts/ui"
mkdir -p "$XDG_DATA_HOME/own-player" "$OWN_PLAYER_SCREENSHOTS"
# Deterministic LRCLIB fixture: UI tests never send requests to the internet.
mkdir -p "$fixture/bin"
cat > "$fixture/bin/curl" <<'PYHTTP'
#!/usr/bin/python3
import json, sys
params = dict(arg.split('=', 1) for arg in sys.argv[1:] if '=' in arg and not arg.startswith('http'))
title = params.get('track_name', 'Song')
result = dict(trackName=title, artistName=params.get('artist_name', ''), duration=20, syncedLyrics='[00:00]' + title)
print(json.dumps(result if sys.argv[-1].endswith('/get') else [result]))
print('200', end='')
PYHTTP
chmod +x "$fixture/bin/curl"
export PATH="$fixture/bin:$PATH"
ffmpeg -v error -f lavfi -i anullsrc=r=8000:cl=mono -t 20 \
  -metadata title='Там, где тихо' -metadata artist='Северный берег' -metadata album='Комната с видом' "$fixture/track.wav"
printf '[00:00]Город засыпает за окном\n[00:04]Свет ложится на ладони\n[00:08]Мы останемся вдвоём\n[00:12]В этом тихом разговоре\n' > "$fixture/track.lrc"
ffmpeg -v error -f lavfi -i 'color=c=0x344b40:s=80x80' -vf 'drawbox=x=15:y=15:w=50:h=50:color=0xbdd3a8:t=2' -frames:v 1 "$fixture/cover.png"
python3 - "$fixture" <<'PY'
import json, os, sys
from pathlib import Path
root = Path(sys.argv[1])
track = str(root / 'track.wav')
tracks = [track]
import wave
with wave.open(track) as audio:
    params, frames = audio.getparams(), audio.readframes(audio.getnframes())
for name in ['Тихий океан - Медленный свет', 'Вторая комната - После дождя', 'Маяк - До утра', 'Линии - Между нами', 'Север - Последний трамвай']:
    path = root / (name + '.wav')
    with wave.open(str(path), "wb") as audio:
        audio.setparams(params)
        audio.writeframes(frames)
    tracks.append(str(path))
(root / 'data/own-player/session.json').write_text(json.dumps(dict(paths=tracks, current_path=track, shuffle=False, repeat=0, online_covers=False, volume=0, speed=1)))
PY
xvfb-run -e /tmp/own-player-xvfb.log -a -s '-screen 0 1160x780x24' cargo test --offline gtk_smoke -- --ignored --test-threads=1 --nocapture
