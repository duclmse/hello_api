#!/usr/bin/env bash
source ./sample.env

# PUT update
curl -X PUT "${BASE_URL}/put" \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer ${TOKEN}" \
  -d "{\"id\": ${USER_ID}, \"name\": \"Alice Updated\"}"
