#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root_dir="$(cd "${script_dir}/.." && pwd)"
out_jar="${root_dir}/inputs/photonvision-linuxarm64.jar"
mode="${GAIA_INPUT_PV_JAR_SOURCE:-release}"
driver_mode="${GAIA_INPUT_PV_DRIVER_SOURCE:-off}"

if [[ -f "${out_jar}" && "${GAIA_FORCE_FETCH_PV_JAR:-0}" != "1" ]]; then
  echo "PhotonVision jar already present: ${out_jar}"
  exit 0
fi

mkdir -p "$(dirname "${out_jar}")"

case "${mode}" in
  release)
    pv_url="${PHOTONVISION_JAR_URL:-https://github.com/PhotonVision/photonvision/releases/latest/download/photonvision-linuxarm64.jar}"
    curl -fL "${pv_url}" -o "${out_jar}"
    echo "Downloaded PhotonVision jar from release URL: ${pv_url}"
    ;;
  local)
    if [[ -z "${PHOTONVISION_JAR_PATH:-}" ]]; then
      echo "local mode requires PHOTONVISION_JAR_PATH=/abs/path/to/photonvision-linuxarm64.jar" >&2
      exit 1
    fi
    if [[ ! -f "${PHOTONVISION_JAR_PATH}" ]]; then
      echo "PHOTONVISION_JAR_PATH does not exist: ${PHOTONVISION_JAR_PATH}" >&2
      exit 1
    fi
    cp -f "${PHOTONVISION_JAR_PATH}" "${out_jar}"
    echo "Copied PhotonVision jar from PHOTONVISION_JAR_PATH"
    ;;
  repo)
    if [[ "${driver_mode}" == "off" ]]; then
      cat >&2 <<'MSG'
pv_jar_source=repo requires pv_driver_source=release|local|repo.
Use CLI flags:
  --set pv_jar_source=repo --set pv_driver_source=release
MSG
      exit 1
    fi

    repo_dir="${PHOTONVISION_REPO_DIR:-}"
    if [[ -z "${repo_dir}" || ! -d "${repo_dir}" ]]; then
      echo "repo mode requires PHOTONVISION_REPO_DIR to point to a local PhotonVision checkout" >&2
      exit 1
    fi

    (
      cd "${repo_dir}"
      ./gradlew :photon-server:shadowJar
    )

    candidate="$(find "${repo_dir}" -type f \( -name "photonvision-linuxarm64*.jar" -o -name "photonvision*.jar" \) | head -n 1 || true)"
    if [[ -z "${candidate}" || ! -f "${candidate}" ]]; then
      echo "failed to locate built PhotonVision jar under ${repo_dir}" >&2
      exit 1
    fi
    cp -f "${candidate}" "${out_jar}"
    echo "Copied PhotonVision jar built from repo: ${candidate}"
    ;;
  *)
    echo "unsupported pv_jar_source mode '${mode}' (expected: release|local|repo)" >&2
    exit 1
    ;;
esac
