#!/usr/bin/env bash
source ./sample.env

# GET anything
curl "${BASE_URL}/anything" \
  -H "Accept: application/json"
