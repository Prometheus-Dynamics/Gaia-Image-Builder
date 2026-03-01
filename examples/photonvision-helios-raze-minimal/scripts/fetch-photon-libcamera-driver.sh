#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root_dir="$(cd "${script_dir}/.." && pwd)"
out_jar="${root_dir}/inputs/photon-libcamera-gl-driver.jar"
mode="${GAIA_INPUT_PV_DRIVER_SOURCE:-off}"

if [[ "${mode}" == "off" ]]; then
  echo "pv_driver_source=off; driver artifact disabled"
  exit 0
fi

if [[ -f "${out_jar}" && "${GAIA_FORCE_FETCH_DRIVER_JAR:-0}" != "1" ]]; then
  echo "Driver jar already present: ${out_jar}"
  exit 0
fi

mkdir -p "$(dirname "${out_jar}")"

case "${mode}" in
  release)
    driver_url="${PHOTON_LIBCAMERA_DRIVER_JAR_URL:-}"
    if [[ -z "${driver_url}" ]]; then
      echo "release mode requires PHOTON_LIBCAMERA_DRIVER_JAR_URL=https://.../photon-libcamera-gl-driver.jar" >&2
      exit 1
    fi
    curl -fL "${driver_url}" -o "${out_jar}"
    echo "Downloaded driver jar from release URL: ${driver_url}"
    ;;
  local)
    if [[ -z "${PHOTON_LIBCAMERA_DRIVER_JAR_PATH:-}" ]]; then
      echo "local mode requires PHOTON_LIBCAMERA_DRIVER_JAR_PATH=/abs/path/to/photon-libcamera-gl-driver.jar" >&2
      exit 1
    fi
    if [[ ! -f "${PHOTON_LIBCAMERA_DRIVER_JAR_PATH}" ]]; then
      echo "PHOTON_LIBCAMERA_DRIVER_JAR_PATH does not exist: ${PHOTON_LIBCAMERA_DRIVER_JAR_PATH}" >&2
      exit 1
    fi
    cp -f "${PHOTON_LIBCAMERA_DRIVER_JAR_PATH}" "${out_jar}"
    echo "Copied driver jar from PHOTON_LIBCAMERA_DRIVER_JAR_PATH"
    ;;
  repo)
    repo_dir="${PHOTON_LIBCAMERA_DRIVER_REPO_DIR:-}"
    if [[ -z "${repo_dir}" || ! -d "${repo_dir}" ]]; then
      echo "repo mode requires PHOTON_LIBCAMERA_DRIVER_REPO_DIR to point to a local checkout" >&2
      exit 1
    fi
    (
      cd "${repo_dir}"
      ./gradlew shadowJar
    )
    candidate="$(find "${repo_dir}" -type f -name "*photon-libcamera-gl-driver*.jar" | head -n 1 || true)"
    if [[ -z "${candidate}" || ! -f "${candidate}" ]]; then
      echo "failed to locate built driver jar under ${repo_dir}" >&2
      exit 1
    fi
    cp -f "${candidate}" "${out_jar}"
    echo "Copied driver jar built from repo: ${candidate}"
    ;;
  *)
    echo "unsupported pv_driver_source mode '${mode}' (expected: off|release|local|repo)" >&2
    exit 1
    ;;
esac
