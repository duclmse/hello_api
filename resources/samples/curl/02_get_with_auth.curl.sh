#!/usr/bin/env bash
source ./sample.env

# GET with bearer auth
curl "${BASE_URL}/bearer" \
  -H "Authorization: Bearer ${TOKEN}"
