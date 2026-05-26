#!/usr/bin/env bash
source ./sample.env

# Upload raw file body (sends the file contents as the request body)
curl -X POST "${BASE_URL}/post" \
  -H "Content-Type: application/json" \
  --data-binary @../_payload/upload_payload.json
