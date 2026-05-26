#!/usr/bin/env bash
source ./sample.env

# Headers echo
curl "${BASE_URL}/headers" \
  -H "X-Custom-Header: hello" \
  -H "X-Request-Id: req-001"
