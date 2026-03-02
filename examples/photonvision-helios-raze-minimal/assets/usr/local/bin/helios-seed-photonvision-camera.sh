#!/bin/sh
set -eu

CONFIG_DIR="/opt/photonvision/photonvision_config"
DB_PATH="${CONFIG_DIR}/photon.sqlite"
FOV_DEFAULT="${HELIOS_CAMERA_FOV:-}"

if [ ! -f "${DB_PATH}" ]; then
  exit 0
fi

if ! command -v sqlite3 >/dev/null 2>&1; then
  exit 0
fi

if ! command -v python3 >/dev/null 2>&1; then
  exit 0
fi

camera_count="$(sqlite3 "${DB_PATH}" "SELECT COUNT(*) FROM cameras;" 2>/dev/null || true)"

camera_list="$(
  rpicam-hello --list-cameras 2>/dev/null || \
  rpicam-vid --list-cameras 2>/dev/null || \
  rpicam-still --list-cameras 2>/dev/null || \
  libcamera-hello --list-cameras 2>/dev/null || \
  libcamera-vid --list-cameras 2>/dev/null || \
  libcamera-still --list-cameras 2>/dev/null || true
)"

if printf '%s' "${camera_list}" | grep -qi "no cameras available"; then
  camera_list=""
fi

set +e
camera_info="$(CAMERA_LIST="${camera_list}" python3 - <<'PY'
import os
import re
import sys

text = os.environ.get("CAMERA_LIST", "")
for line in text.splitlines():
    match = re.match(r"^\s*\d+\s*:\s*(.*?)\s*(?:\[.*\])?\s*\((.+)\)\s*$", line)
    if match:
        name = match.group(1).strip()
        path = match.group(2).strip()
        if name and path:
            print(f"{name}|{path}")
            sys.exit(0)
sys.exit(1)
PY
)"
set -e

if [ -z "${camera_info}" ]; then
  # Fallback for systems where libcamera apps are too old to enumerate cameras.
  set +e
  camera_info="$(python3 - <<'PY'
import re
import subprocess
import sys

try:
    text = subprocess.check_output(
        ["media-ctl", "-p", "-d", "/dev/media0"],
        stderr=subprocess.DEVNULL,
        text=True,
    )
except Exception:
    sys.exit(1)

bus_info = ""
for line in text.splitlines():
    if line.strip().startswith("bus info"):
        bus_info = line.split("bus info", 1)[1].strip()
        break

sensor_name = ""
for line in text.splitlines():
    m = re.search(r"entity\s+\d+:\s+(.+?)\s+\(\d+ pad", line)
    if not m:
        continue
    name = m.group(1).strip()
    # Prefer known sensor prefixes if present.
    if name.startswith("ov") or name.startswith("imx") or name.startswith("ar"):
        sensor_name = name
        break

if not sensor_name:
    for line in text.splitlines():
        m = re.search(r"entity\s+\d+:\s+(.+?)\s+\(\d+ pad", line)
        if m:
            sensor_name = m.group(1).strip()
            break

if sensor_name and bus_info:
    # Drop trailing I2C address (e.g. "10-0060") if present.
    sensor_name = re.sub(r"\s+\d+-[0-9a-fA-F]{4}$", "", sensor_name).strip()
    print(f"{sensor_name}|{bus_info}")
    sys.exit(0)
sys.exit(1)
PY
)"
  set -e
fi

if [ -z "${camera_info}" ]; then
  exit 0
fi

base_name="${camera_info%%|*}"
camera_path="${camera_info#*|}"
raw_base_name="${base_name}"
camera_base_lc="$(printf '%s' "${base_name}" | tr '[:upper:]' '[:lower:]')"
case "${camera_base_lc}" in
  ov*|imx*|ar*)
    base_name="$(printf '%s' "${base_name}" | tr '[:lower:]' '[:upper:]')"
    ;;
esac
safe_name="$(printf '%s' "${raw_base_name}" | tr ' ' '_' | tr -cd 'A-Za-z0-9_-')"

if [ -z "${safe_name}" ]; then
  safe_name="Camera0"
fi

tmp_dir="$(mktemp -d)"
trap 'rm -rf "${tmp_dir}"' EXIT

PV_BASE_NAME="${base_name}" PV_CAMERA_PATH="${camera_path}" PV_UNIQUE_NAME="${safe_name}" \
PV_FOV="${FOV_DEFAULT}" \
python3 - <<'PY' > "${tmp_dir}/config.json"
import json
import os

config = {
    "baseName": os.environ["PV_BASE_NAME"],
    "uniqueName": os.environ["PV_UNIQUE_NAME"],
    "nickname": os.environ["PV_UNIQUE_NAME"],
    "FOV": float(os.environ.get("PV_FOV", "70.0") or "70.0"),
    "path": os.environ["PV_CAMERA_PATH"],
    "cameraType": "ZeroCopyPicam",
    "usbVID": 0,
    "usbPID": 0,
    "calibrations": [],
    "currentPipelineIndex": 0,
}

print(json.dumps(config, indent=2))
PY

printf '[ "DriverModePipelineSettings", {} ]\n' > "${tmp_dir}/drivermode.json"
printf '[]\n' > "${tmp_dir}/pipeline.json"
printf '[]\n' > "${tmp_dir}/otherpaths.json"

sqlite3 "${DB_PATH}" <<'SQL'
CREATE TABLE IF NOT EXISTS global (
 filename TINYTEXT PRIMARY KEY,
 contents mediumtext NOT NULL
);
CREATE TABLE IF NOT EXISTS cameras (
 unique_name TINYTEXT PRIMARY KEY,
 config_json text NOT NULL,
 drivermode_json text NOT NULL,
 pipeline_jsons mediumtext NOT NULL
);
SQL

have_otherpaths="$(sqlite3 "${DB_PATH}" "SELECT name FROM pragma_table_info('cameras') WHERE name='otherpaths_json';" 2>/dev/null || true)"
if [ -z "${have_otherpaths}" ]; then
  sqlite3 "${DB_PATH}" "ALTER TABLE cameras ADD COLUMN otherpaths_json TEXT NOT NULL DEFAULT '[]';"
fi

schema_info="$(sqlite3 "${DB_PATH}" "SELECT name, notnull, dflt_value FROM pragma_table_info('cameras') WHERE name='otherpaths_json';" 2>/dev/null || true)"
schema_notnull="$(printf '%s' "${schema_info}" | cut -d'|' -f2)"
schema_default="$(printf '%s' "${schema_info}" | cut -d'|' -f3)"
if [ "${schema_notnull}" = "1" ] && [ -z "${schema_default}" ]; then
  sqlite3 "${DB_PATH}" <<'SQL'
BEGIN;
CREATE TABLE IF NOT EXISTS cameras_new (
 unique_name TINYTEXT PRIMARY KEY,
 config_json text NOT NULL,
 drivermode_json text NOT NULL,
 otherpaths_json TEXT NOT NULL DEFAULT '[]',
 pipeline_jsons mediumtext NOT NULL
);
INSERT INTO cameras_new (unique_name, config_json, drivermode_json, otherpaths_json, pipeline_jsons)
SELECT unique_name, config_json, drivermode_json, COALESCE(NULLIF(otherpaths_json, ''), '[]'), pipeline_jsons
FROM cameras;
DROP TABLE cameras;
ALTER TABLE cameras_new RENAME TO cameras;
COMMIT;
SQL
fi

if [ -n "${camera_count}" ] && [ "${camera_count}" -gt 0 ]; then
  PV_DB_PATH="${DB_PATH}" PV_BASE_NAME="${base_name}" PV_CAMERA_PATH="${camera_path}" PV_FOV="${FOV_DEFAULT}" python3 - <<'PY'
import json
import os
import sqlite3

db_path = os.environ["PV_DB_PATH"]
base_name = os.environ["PV_BASE_NAME"]
camera_path = os.environ["PV_CAMERA_PATH"]
camera_fov_env = os.environ.get("PV_FOV", "").strip()
camera_fov = float(camera_fov_env) if camera_fov_env else None

conn = sqlite3.connect(db_path)
rows = conn.execute("SELECT unique_name, config_json FROM cameras").fetchall()

for unique_name, config_text in rows:
    try:
        config = json.loads(config_text)
    except Exception:
        continue

    changed = False
    if camera_fov is not None and config.get("FOV") != camera_fov:
        config["FOV"] = camera_fov
        changed = True

    csi = (config.get("matchedCameraInfo") or {}).get("PVCSICameraInfo")
    if isinstance(csi, dict):
        if csi.get("path") != camera_path:
            csi["path"] = camera_path
            changed = True
        if csi.get("uniquePath") != camera_path:
            csi["uniquePath"] = camera_path
            changed = True
        if csi.get("baseName") != base_name:
            csi["baseName"] = base_name
            changed = True
    elif config.get("cameraType") == "ZeroCopyPicam":
        if config.get("path") != camera_path:
            config["path"] = camera_path
            changed = True
        if config.get("baseName") != base_name:
            config["baseName"] = base_name
            changed = True

    if changed:
        conn.execute(
            "UPDATE cameras SET config_json = ? WHERE unique_name = ?",
            (json.dumps(config, indent=2), unique_name),
        )

conn.commit()
conn.close()
PY
  exit 0
fi

sqlite3 "${DB_PATH}" <<SQL
INSERT OR IGNORE INTO cameras (unique_name, config_json, drivermode_json, otherpaths_json, pipeline_jsons)
VALUES ('${safe_name}', readfile('${tmp_dir}/config.json'), readfile('${tmp_dir}/drivermode.json'), readfile('${tmp_dir}/otherpaths.json'), readfile('${tmp_dir}/pipeline.json'));
SQL
