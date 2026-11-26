#!/bin/zsh
# Xcode build phase (project.yml postCompileScripts): put the Google Drive Desktop OAuth client into the app bundle
# (Contents/Resources/gdrive-client.json), where GDriveClient.bundled reads it (docs/19 7-0절, 7-1절 1).
# Google requires the Desktop client's secret and does not treat it as confidential, so the app carries it, but it is
# not in git: it comes from the JSON Google Cloud gave (~/.config/google/tacie-drive-client.json, or
# TACIE_GDRIVE_CLIENT). Without that file the build goes on and the Drive card says the connection is not ready yet.
# Never prints the values.
set -e
src=${TACIE_GDRIVE_CLIENT:-$HOME/.config/google/tacie-drive-client.json}
dest="${TARGET_BUILD_DIR:?}/${UNLOCALIZED_RESOURCES_FOLDER_PATH:?}/gdrive-client.json"
# the open edition never carries Tacie's Drive client; a person brings their own (docs/41 0절)
if [[ "${TACIE_EDITION:-official}" == open ]]; then
  rm -f "$dest"
  exit 0
fi
if [[ ! -r "$src" ]]; then
  rm -f "$dest"
  echo "warning: no Google Drive client ($src): the Drive connection will say it is not ready"
  exit 0
fi
id=$(plutil -extract installed.client_id raw -o - "$src")
secret=$(plutil -extract installed.client_secret raw -o - "$src")
# Google's ids and secrets are letters, digits, '-', '_' and '.': nothing to escape in JSON
if [[ ! "$id$secret" =~ '^[A-Za-z0-9._-]+$' ]]; then
  echo "error: $src: client_id or client_secret has unexpected characters" >&2
  exit 1
fi
mkdir -p "${dest:h}"
umask 022
printf '{"client_id": "%s", "client_secret": "%s"}\n' "$id" "$secret" > "$dest"
