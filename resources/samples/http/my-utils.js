export function makeSignature() {
  return crypto.hmac
    .sha256()
    .withTextSecret(request.environment.get("secret"))
    .updateWithText(request.body.tryGetSubstituted())
    .digest()
    .toHex();
}

export function findSignature(response) {
  return response.headers["X-My-Signature"];
}
