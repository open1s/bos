#!/usr/bin/env bash
set -euo pipefail

REPO_OWNER="open1s"
REPO_NAME="bos"
HEAD_BRANCH="plan-a-release"
BASE_BRANCH="main"
TITLE="Release Plan A: QA & Publish Readiness"
BODY_FILE="PLAN_A_RELEASE.md"

if [ -z "${GITHUB_TOKEN:-}" ]; then
  echo "ERROR: GITHUB_TOKEN is not set. Please export GITHUB_TOKEN with a GitHub access token that has repo scope." >&2
  exit 1
fi

if [ ! -f "$BODY_FILE" ]; then
  echo "ERROR: Body file '$BODY_FILE' not found in repo root." >&2
  exit 1
fi

BODY_CONTENT=$(python - << 'PY'
import json, sys
with open("PLAN_A_RELEASE.md", 'r') as f:
  text = f.read()
print(json.dumps(text))
PY
)

REPO_API_URL="https://api.github.com/repos/${REPO_OWNER}/${REPO_NAME}/pulls"

curl -s -X POST -H "Authorization: token ${GITHUB_TOKEN}" -H "Accept: application/vnd.github.v3+json" \
  -d "{\"title\":\"${TITLE}\",\"head\":\"${HEAD_BRANCH}\",\"base\":\"${BASE_BRANCH}\",\"body\":${BODY_CONTENT}}" \
  ${REPO_API_URL}
