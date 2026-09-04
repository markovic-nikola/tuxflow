# Sound credits

Notification sounds bundled with TuxFlow.

| Source | License | Files |
| --- | --- | --- |
| [UI SFX](https://github.com/romainsimon/uisfx) by Yuki Capital (v0.4.0, procedurally generated) | CC0 1.0 (Public Domain) | `{pack}-{cue}.ogg` — 9 packs × 8 cues |

Packs: minimal, soft, glass, arcade, organic, dreamy, scifi, rubber, cinematic.
Cues: notification, success, error, warning, badge, reward, achievement, checkpoint.

Only the audio files under uisfx's `packages/uisfx/sounds` are used. The
upstream Ogg files are Opus renders peaking around -7 dBFS (uisfx's own player
applies a per-cue gain); ours are peak-normalized to -1 dBFS and re-encoded as
Ogg Vorbis (ffmpeg, `-af volume=<gain>dB -c:a libvorbis -q:a 6`) so they sit at
the level of the Kenney set they replaced and need no Opus support in
libsndfile. The uisfx code (MIT) is not included.

CC0 requires no attribution, but this file is kept as a record of provenance.
