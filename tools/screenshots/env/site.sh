#!/usr/bin/env bash
# Assemble out/site/ with the file names the website expects, from the shots
# the tapes wrote into out/. Runs in the recording image (it has ffmpeg).
set -uo pipefail
cd /opt/shots/out
rm -rf site
mkdir -p site/screenshots site/themes

missing=()
take() {  # take <source> <site name>
    if [ -f "$1" ]; then cp "$1" "site/screenshots/$2"; else missing+=("$2 ($1)"); fi
}
take overview.png overview.png
take agent-done.png agent.png
take agent.gif agent.gif
take file-manager.png file-manager.png
take detach-attached.png detach.png
take detach.gif detach.gif
take git-log.png git.png
take database.png db.png
take monitor.png monitor.png
take markdown.png markdown.png
take mermaid.png mermaid.png
take hex.png hex.png
take terminal.png terminal.png
take debug.png debug.png
take debug.gif debug.gif
take tour.gif tour.gif
take ghostty/image.png image.png

ff() { ffmpeg -loglevel error -y "$@"; }

# Theme thumbnails at half size; the gallery shows them small.
for shot in themes/*.png; do
    [ -f "$shot" ] && ff -i "$shot" -vf scale=820:-1 "site/themes/$(basename "$shot")"
done

# themes.png: a 3x2 collage across the dark, light and retro families.
collage=(norton-commander dracula ayu-light matrix far-manager solarized-light)
inputs=() ok=1
for id in "${collage[@]}"; do
    [ -f "themes/$id.png" ] || ok=0
    inputs+=(-i "themes/$id.png")
done
if [ $ok = 1 ]; then
    ff "${inputs[@]}" -filter_complex \
        "$(for i in 0 1 2 3 4 5; do printf '[%d:v]scale=820:-1[t%d];' $i $i; done)[t0][t1][t2][t3][t4][t5]xstack=inputs=6:layout=0_0|w0_0|w0+w1_0|0_h0|w0_h0|w0+w1_h0" \
        site/screenshots/themes.png
else
    missing+=("themes.png (theme shots)")
fi

# og.png: 1200x630, the top of the overview.
if [ -f overview.png ]; then
    ff -i overview.png -vf "scale=1200:-1,crop=1200:630:0:0" site/og.png
else
    missing+=("og.png (overview.png)")
fi

if [ ${#missing[@]} -gt 0 ]; then
    printf 'site: missing %s\n' "${missing[@]}" >&2
fi
echo "site: $(find site -name '*.png' -o -name '*.gif' | wc -l) files"
