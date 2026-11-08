#!/bin/sh
# Buildroot post-build for the AI-CAM, after the K230D's own: records the
# fitted lens (the fourth script argument), the only thing that differs
# between the 72 and 140 degree boards, as data for what reads the camera.
set -e
lens="${4:?usage: post-build.sh <target dir> <dts> <sdk root> <lens degrees>}"
printf 'LENS_FOV_DEGREES=%s\n' "$lens" > "$TARGET_DIR/etc/camera-lens"
