#!/bin/bash
# Regenerates the decoder fixtures with ffmpeg (6.x, with libmp3lame and libvorbis).
# The files are committed so that `cargo test` needs no external tools; tests/decode_formats.rs
# rebuilds the same signal in code and compares.
#
#   stereo: left 0.5*sin(2*pi*440*t), right 0.25*sin(2*pi*1000*t)   mono: the left channel
#   lossless/PCM fixtures: 0.1 s (4410 frames at 44.1 kHz)          lossy fixtures: 0.5 s
set -euo pipefail
cd "$(dirname "$0")"
ff() { ffmpeg -hide_banner -loglevel error -y "$@"; }
stereo() { echo "aevalsrc=0.5*sin(2*PI*440*t)|0.25*sin(2*PI*1000*t):s=$1:d=$2"; }
mono() { echo "aevalsrc=0.5*sin(2*PI*440*t):s=$1:d=$2"; }
bitexact="-fflags +bitexact -flags:a +bitexact -map_metadata -1"

# WAV
ff -f lavfi -i "$(stereo 44100 0.1)" $bitexact -c:a pcm_s16le wav_s16.wav
ff -f lavfi -i "$(stereo 44100 0.1)" $bitexact -c:a pcm_s24le wav_s24_extensible.wav
ff -f lavfi -i "$(stereo 48000 0.1)" $bitexact -c:a pcm_s32le wav_s32_48k.wav
ff -f lavfi -i "$(stereo 44100 0.1)" $bitexact -c:a pcm_f32le wav_f32.wav
ff -f lavfi -i "$(mono 44100 0.1)" $bitexact -c:a pcm_u8 wav_u8_mono.wav
ff -f lavfi -i "$(mono 22050 0.1)" $bitexact -c:a pcm_mulaw wav_ulaw_mono_22k.wav
ff -f lavfi -i "$(mono 22050 0.1)" $bitexact -c:a pcm_alaw wav_alaw_mono_22k.wav
ff -f lavfi -i "$(stereo 44100 0.5)" $bitexact -c:a adpcm_ima_wav wav_adpcm_ima.wav
ff -f lavfi -i "$(stereo 44100 0.5)" $bitexact -c:a adpcm_ms wav_adpcm_ms.wav
# AIFF / AIFC
ff -f lavfi -i "$(stereo 44100 0.1)" $bitexact -c:a pcm_s16be aiff_s16.aiff
ff -f lavfi -i "$(stereo 96000 0.05)" $bitexact -c:a pcm_s24be aiff_s24_96k.aiff
ff -f lavfi -i "$(stereo 44100 0.1)" $bitexact -c:a pcm_s16le aifc_sowt.aifc
ff -f lavfi -i "$(mono 44100 0.1)" $bitexact -c:a pcm_f32be aifc_fl32_mono.aifc
# CAF
ff -f lavfi -i "$(stereo 44100 0.1)" $bitexact -c:a pcm_s16be caf_s16.caf
ff -f lavfi -i "$(stereo 48000 0.1)" $bitexact -c:a pcm_f32le caf_f32_48k.caf
ff -f lavfi -i "$(stereo 44100 0.1)" $bitexact -c:a alac caf_alac.caf
# FLAC
ff -f lavfi -i "$(stereo 44100 0.1)" $bitexact -c:a flac -sample_fmt s16 flac_16.flac
ff -f lavfi -i "$(stereo 96000 0.05)" $bitexact -c:a flac -sample_fmt s32 -bits_per_raw_sample 24 flac_24_96k.flac
# Apple Lossless and AAC in MP4, raw AAC
ff -f lavfi -i "$(stereo 44100 0.1)" $bitexact -c:a alac m4a_alac.m4a
ff -f lavfi -i "$(stereo 44100 0.5)" $bitexact -c:a aac -b:a 192k m4a_aac.m4a
ff -f lavfi -i "$(mono 48000 0.5)" $bitexact -c:a aac -b:a 96k m4a_aac_mono_48k.m4a
ff -f lavfi -i "$(stereo 44100 0.5)" $bitexact -c:a aac -b:a 192k -f adts aac_adts.aac
# MPEG audio
ff -f lavfi -i "$(stereo 44100 0.5)" $bitexact -c:a libmp3lame -b:a 192k mp3_cbr.mp3
ff -f lavfi -i "$(mono 48000 0.5)" $bitexact -c:a libmp3lame -q:a 4 mp3_vbr_mono_48k.mp3
ff -f lavfi -i "$(stereo 44100 0.5)" $bitexact -c:a mp2 -b:a 192k mp2.mp2
# Ogg Vorbis
ff -f lavfi -i "$(stereo 44100 0.5)" $bitexact -c:a libvorbis -q:a 6 ogg_vorbis.ogg
ls -l
