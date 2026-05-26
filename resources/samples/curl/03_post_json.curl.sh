#!/usr/bin/env bash
source ./sample.env

# POST JSON body
curl -X POST "${BASE_URL}/post" \
  -H "Content-Type: application/json" \
  -H "Accept: application/json" \
  -d "{\"id\": ${USER_ID}, \"name\": \"Alice\", \"email\": \"alice@example.com\"}"
