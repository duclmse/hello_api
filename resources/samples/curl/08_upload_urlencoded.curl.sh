#!/usr/bin/env bash
source ./sample.env

# URL-encoded form upload
curl -X POST "${BASE_URL}/post" \
  --data-urlencode "username=${USER_ID}" \
  --data-urlencode "name=Alice" \
  --data-urlencode "role=admin"
