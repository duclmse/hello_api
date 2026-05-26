#!/usr/bin/env bash
source ./sample.env

# Multipart form upload — text field + file attachment
curl -X POST "${BASE_URL}/post" \
  -F "title=File upload demo" \
  -F "payload=@../_payload/upload_payload.json;type=application/json"
