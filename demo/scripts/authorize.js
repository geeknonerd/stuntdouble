// GET /oauth2/authorize — plans/oauth-authorization-mock.md §7.1.
//
// Validates the authorize request parameters and, on success, issues a fresh
// one-time authorization code and redirects to the registered redirect_uri.
// The code is stored via ctx.state with a 60-second TTL and is never accepted
// from client input. This script does not read any request-signature headers.
const CLIENT_ID = "demo-client";
const REDIRECT_URI = "http://127.0.0.1:18081/callback";

function sendError(status, code) {
  ctx.respond(
    status,
    { "Content-Type": "application/json; charset=utf-8" },
    JSON.stringify({ error: code, error_description: code })
  );
}

function validateParams() {
  var query = ctx.request.query;

  // response_type must be "code"
  if (query.response_type !== "code") {
    sendError(400, "invalid_request");
    return false;
  }

  // client_id must match
  if (query.client_id !== CLIENT_ID) {
    sendError(400, "invalid_request");
    return false;
  }

  // redirect_uri must match exactly
  if (query.redirect_uri !== REDIRECT_URI) {
    sendError(400, "invalid_request");
    return false;
  }

  // state: 1 to 128 visible ASCII characters
  var state = query.state;
  if (
    typeof state !== "string" ||
    state.length < 1 ||
    state.length > 128
  ) {
    sendError(400, "invalid_request");
    return false;
  }
  // visible ASCII: 0x20 (space) to 0x7e (~)
  for (var i = 0; i < state.length; i += 1) {
    var c = state.charCodeAt(i);
    if (c < 0x20 || c > 0x7e) {
      sendError(400, "invalid_request");
      return false;
    }
  }

  return true;
}

function issueCode(state) {
  // Generate 32 random bytes = 64 hex chars
  var bytes = ctx.crypto.randomBytes(32);
  var code = Array.prototype.map.call(bytes, function (b) {
    return ("0" + b.toString(16)).slice(-2);
  }).join("");

  // Store with 60-second TTL; key prefix to avoid collisions
  var key = "oauth:code:" + code;
  var value = { state: state };
  ctx.state.set(key, value, { ttl_ms: 60000 });

  // Build Location with code and percent-encoded state
  var encodedState = encodeURIComponent(state);
  var location = REDIRECT_URI + "?code=" + code + "&state=" + encodedState;

  ctx.respond(302, {
    Location: location,
    "Cache-Control": "no-store",
  }, "");
}

if (!validateParams()) {
  // Error already sent
} else {
  var state = ctx.request.query.state;
  issueCode(state);
}
