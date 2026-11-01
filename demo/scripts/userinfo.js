// GET /oauth2/userinfo — plans/oauth-authorization-mock.md §7.3.
//
// Validation order (contract §6.4 and §8.3): signature first, then the
// Authorization header and token lookup. Query parameters are signed but never
// read as business input. Every Route script runs in its own realm, so the
// signature helpers below mirror token.js rather than sharing a module.
const SIG_KEY_ID = "mock-key-01";
const SIG_VERSION = "1";
const SIGNING_SECRET = "demo-signing-secret";
const SIG_WINDOW_SECONDS = 300;
const REPLAY_TTL_MS = 600000;
const TEST_USER = {
  sub: "u-10001",
  name: "Alice",
  org_id: "org-10",
  org_name: "Example Org",
};

function toHex(bytes) {
  var out = "";
  for (var i = 0; i < bytes.length; i += 1) {
    out += (bytes[i] < 16 ? "0" : "") + bytes[i].toString(16);
  }
  return out;
}

function hexToBytes(hex) {
  var out = new Array(hex.length / 2);
  for (var i = 0; i < out.length; i += 1) {
    out[i] = hexNibble(hex.charCodeAt(i * 2)) * 16 + hexNibble(hex.charCodeAt(i * 2 + 1));
  }
  return out;
}

// ASCII code unit to its 0-15 hex value; callers already validated the input.
function hexNibble(code) {
  if (code >= 0x61) {
    return code - 0x57;
  }
  if (code >= 0x41) {
    return code - 0x37;
  }
  return code - 0x30;
}

function isLowerHex(text, length) {
  if (typeof text !== "string" || text.length !== length) {
    return false;
  }
  for (var i = 0; i < text.length; i += 1) {
    var c = text.charCodeAt(i);
    var ok = (c >= 0x30 && c <= 0x39) || (c >= 0x61 && c <= 0x66);
    if (!ok) {
      return false;
    }
  }
  return true;
}

function isDecimalSeconds(text) {
  // Keep the conversion exact for JavaScript numbers; Unix seconds fit in
  // 10 digits until 2286, so longer values are malformed rather than useful.
  if (typeof text !== "string" || text.length === 0 || text.length > 10) {
    return false;
  }
  for (var i = 0; i < text.length; i += 1) {
    var c = text.charCodeAt(i);
    if (c < 0x30 || c > 0x39) {
      return false;
    }
  }
  return true;
}

// §6.2: percent-encode with uppercase hex, spaces as %20, sorted by encoded
// name then encoded value. Returns null when the raw query cannot be decoded,
// so the caller fails closed instead of signing a guessed value.
function canonicalQuery(rawQuery) {
  if (typeof rawQuery !== "string") {
    return null;
  }
  if (rawQuery === "") {
    return "";
  }
  var pairs = [];
  var parts = rawQuery.split("&");
  for (var i = 0; i < parts.length; i += 1) {
    if (parts[i] === "") {
      continue;
    }
    var eq = parts[i].indexOf("=");
    var rawName = eq < 0 ? parts[i] : parts[i].slice(0, eq);
    var rawValue = eq < 0 ? "" : parts[i].slice(eq + 1);
    var name = pctDecode(rawName);
    var value = pctDecode(rawValue);
    if (name === null || value === null) {
      return null;
    }
    pairs.push([name, value]);
  }
  var encoded = [];
  for (var j = 0; j < pairs.length; j += 1) {
    encoded.push([pctEncode(pairs[j][0]), pctEncode(pairs[j][1])]);
  }
  encoded.sort(function (a, b) {
    if (a[0] !== b[0]) {
      return a[0] < b[0] ? -1 : 1;
    }
    if (a[1] !== b[1]) {
      return a[1] < b[1] ? -1 : 1;
    }
    return 0;
  });
  var items = [];
  for (var k = 0; k < encoded.length; k += 1) {
    items.push(encoded[k][0] + "=" + encoded[k][1]);
  }
  return items.join("&");
}

function pctEncode(text) {
  // Escape `!'()*` too: §6.2 keeps only RFC 3986 unreserved characters, and
  // uppercase the hex digits of every percent-encoded sequence.
  return encodeURIComponent(text)
    .replace(/[!'()*]/g, function (c) {
      return "%" + c.charCodeAt(0).toString(16).toUpperCase();
    })
    .replace(/%[0-9a-f]{2}/gi, function (m) {
      return m.toUpperCase();
    });
}

function pctDecode(text) {
  try {
    return decodeURIComponent(text);
  } catch (e) {
    return null;
  }
}

function sendError(status, code, description) {
  ctx.respond(
    status,
    {
      "Content-Type": "application/json; charset=utf-8",
      "Cache-Control": "no-store",
      Pragma: "no-cache",
    },
    JSON.stringify({ error: code, error_description: description })
  );
}

function sigError(description) {
  sendError(
    401,
    "invalid_signature",
    description || "missing or invalid request signature"
  );
}

function tokenError() {
  sendError(401, "invalid_token", "missing or invalid access token");
}

// Returns true when the signature is valid and the request ID was reserved.
// The reservation key is shared with token.js, so one request ID can never be
// accepted twice across both endpoints.
function verifySignature() {
  var headers = ctx.request.headers;
  var keyId = headers["x-sig-key-id"];
  var requestId = headers["x-sig-request-id"];
  var timestamp = headers["x-sig-timestamp"];
  var version = headers["x-sig-version"];
  var signature = headers["x-sig-signature"];
  if (
    keyId === undefined ||
    requestId === undefined ||
    timestamp === undefined ||
    version === undefined ||
    signature === undefined
  ) {
    sigError();
    return false;
  }
  if (!isLowerHex(requestId, 32)) {
    sigError();
    return false;
  }
  if (!isDecimalSeconds(timestamp)) {
    sigError();
    return false;
  }
  if (!isLowerHex(signature, 64)) {
    sigError();
    return false;
  }
  if (keyId !== SIG_KEY_ID || version !== SIG_VERSION) {
    sigError();
    return false;
  }
  var now = Math.floor(Date.now() / 1000);
  // `timestamp` was validated as decimal digits; convert without `parseInt`.
  var sent = 0;
  for (var i = 0; i < timestamp.length; i += 1) {
    sent = sent * 10 + (timestamp.charCodeAt(i) - 0x30);
  }
  if (Math.abs(now - sent) > SIG_WINDOW_SECONDS) {
    sigError("signature timestamp is outside the allowed window");
    return false;
  }
  var replayKey = "oauth:sig:" + requestId;
  if (ctx.state.get(replayKey) !== undefined) {
    sigError();
    return false;
  }
  var body = ctx.request.bodyBytes;
  var bodyBytes = body === null ? new Uint8Array(0) : body;
  var canonicalQueryValue = canonicalQuery(ctx.request.rawQuery);
  if (canonicalQueryValue === null) {
    sigError("request query has malformed percent-encoding");
    return false;
  }
  var canonical =
    "GET\n" +
    ctx.request.path + "\n" +
    canonicalQueryValue + "\n" +
    timestamp + "\n" +
    requestId + "\n" +
    toHex(ctx.crypto.sha256(bodyBytes));
  var expected = toHex(ctx.crypto.hmacSha256(SIGNING_SECRET, canonical));
  if (!ctx.crypto.timingSafeEqual(hexToBytes(expected), hexToBytes(signature))) {
    sigError();
    return false;
  }
  // Only a verified request ID is reserved; a replay then fails here.
  ctx.state.set(replayKey, true, { ttl_ms: REPLAY_TTL_MS });
  return true;
}

// `Authorization: Bearer <token>`; returns null for anything else. The scheme
// is case-insensitive per RFC 7235; the token must be a single non-empty run
// of visible characters.
function bearerToken() {
  var header = ctx.request.headers["authorization"];
  if (typeof header !== "string") {
    return null;
  }
  var space = header.indexOf(" ");
  if (space < 0 || header.slice(0, space).toLowerCase() !== "bearer") {
    return null;
  }
  var token = header.slice(space + 1);
  if (token.length === 0) {
    return null;
  }
  for (var i = 0; i < token.length; i += 1) {
    var c = token.charCodeAt(i);
    if (c < 0x21 || c > 0x7e) {
      return null;
    }
  }
  return token;
}

if (verifySignature()) {
  var token = bearerToken();
  if (token === null) {
    tokenError();
  } else if (ctx.state.get("oauth:token:" + token) === undefined) {
    tokenError();
  } else {
    ctx.respond(
      200,
      {
        "Content-Type": "application/json; charset=utf-8",
        "Cache-Control": "no-store",
        Pragma: "no-cache",
      },
      JSON.stringify(TEST_USER)
    );
  }
}
