#!/usr/bin/env bash
# Installs only this application's two launcher files and its own icon.
set -euo pipefail
ops_project=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
ops_desktop_dir=$(xdg-user-dir DESKTOP)
export OPS_PROJECT="$ops_project" OPS_DESKTOP_DIR="$ops_desktop_dir"
python3 - <<'PY'
import os
from pathlib import Path
project=Path(os.environ['OPS_PROJECT'])
config=Path(os.environ.get('XDG_DATA_HOME',str(Path.home()/'.local/share')))
desktop=Path(os.environ['OPS_DESKTOP_DIR'])
if not desktop.is_absolute():raise SystemExit('Desktop path must be absolute')
def quoted(value):
    if any(c in str(value) for c in '\r\n\0'):raise SystemExit('Unsupported path')
    # Desktop Entry string decoding happens before Exec argument decoding.
    argument='"'+str(value).replace('\\','\\\\').replace('"','\\"').replace('`','\\`').replace('$','\\$').replace('%','%%')+'"'
    return argument.replace('\\','\\\\')
icon=config/'icons/hicolor/128x128/apps/com.aic.ops.png'
icon.parent.mkdir(parents=True,exist_ok=True)
icon.write_bytes((project/'src-tauri/icons/ops-128.png').read_bytes())
command=[str(project/'run.sh')]
overrides=[key+'='+os.environ[key] for key in ['OPSPIER_APPLICATION','OPSPIER_SYSROOT'] if os.environ.get(key)]
if overrides:command=['/usr/bin/env',*overrides,*command]
entry='\n'.join(['[Desktop Entry]','Version=1.0','Type=Application','Name=OpsPier','Comment=SSH、数据库与 Kubernetes 运维工作台','Exec='+' '.join(quoted(part) for part in command),'Icon='+str(icon),'Terminal=false','Categories=Development;System;','StartupWMClass=opspier','StartupNotify=true',''])
for target in [config/'applications/com.aic.ops.desktop',desktop/'com.aic.ops.desktop']:
    target.parent.mkdir(parents=True,exist_ok=True)
    target.write_text(entry,encoding='utf-8');target.chmod(0o755)
print('OpsPier application-menu and desktop launchers installed')
PY
if [[ -n "${DBUS_SESSION_BUS_ADDRESS:-}" ]] && command -v gio >/dev/null; then
  gio set -t string "$ops_desktop_dir/com.aic.ops.desktop" metadata::trusted true >/dev/null 2>&1 || true
fi
