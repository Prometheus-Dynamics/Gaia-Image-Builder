#!/bin/sh
set -eu

JAVA_OPTS="${PHOTONVISION_JAVA_OPTS:--Xmx512m}"
EXTRA_ARGS="${PHOTONVISION_ARGS:-}"

# shellcheck disable=SC2086
exec /usr/bin/java ${JAVA_OPTS} -jar /opt/photonvision/photonvision.jar ${EXTRA_ARGS}
