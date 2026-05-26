#!/usr/bin/env bash
source ./sample.env

# DELETE with bearer auth
curl -X DELETE "${BASE_URL}/delete" \
  -H "Authorization: Bearer ${TOKEN}"