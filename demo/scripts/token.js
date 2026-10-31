// POST /oauth2/token — plans/oauth-authorization-mock.md §7.2.
//
// Validation order (contract §6.4 and §8.3): signature first, then form and
// OAuth parameters, then credentials and grant. The authorization code is
// consumed with one atomic `ctx.state.take`, so concurrent exchanges of the
// same code produce exactly one success.
const CLIENT_ID = "demo-client";
const CLIENT_SECRET = "demo-client-secret";
const REDIRECT_URI = "http://127.0.0.1:18081/callback";
const SIG_KEY_ID = "mock-key-01";
const SIG_VERSION = "1";
const SIGNING_SECRET = "demo-signing-secret";
const SIG_WINDOW_SECONDS = 300;
const REPLAY_TTL_MS = 600000;
const CODE_TTL_MS = 60000;
const TOKEN_TTL_MS = 300000;

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

// application/x-www-form-urlencoded turns "+" into a space; percent escapes
// keep their standard meaning.
function formDecode(text) {
  return pctDecode(text.replace(/\+/g, "%20"));
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

// Returns true when the signature is valid and the request ID was reserved.
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
    "POST\n" +
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

// First value wins for a repeated field. Returns null when the body is not
// readable as UTF-8, and false when percent-decoding fails; both map to 400.
function parseForm() {
  var text = ctx.request.bodyText;
  if (text === null) {
    return null;
  }
  var form = {};
  var parts = text.split("&");
  for (var i = 0; i < parts.length; i += 1) {
    if (parts[i] === "") {
      continue;
    }
    var eq = parts[i].indexOf("=");
    var name = formDecode(eq < 0 ? parts[i] : parts[i].slice(0, eq));
    var value = eq < 0 ? "" : formDecode(parts[i].slice(eq + 1));
    if (name === null || value === null) {
      return false;
    }
    if (!Object.prototype.hasOwnProperty.call(form, name)) {
      form[name] = value;
    }
  }
  return form;
}

function contentTypeIsForm() {
  var raw = ctx.request.headers["content-type"];
  if (typeof raw !== "string") {
    return false;
  }
  return raw.split(";")[0].trim().toLowerCase() === "application/x-www-form-urlencoded";
}

function exchange(form) {
  var code = form.code;
  var codeKey = "oauth:code:" + code;
  // One atomic take: exactly one concurrent exchange can remove the code.
  var entry = ctx.state.take(codeKey);
  if (entry === undefined) {
    sendError(400, "invalid_grant", "authorization code is invalid, expired, or already used");
    return;
  }
  if (entry.redirect_uri !== REDIRECT_URI) {
    sendError(400, "invalid_grant", "redirect_uri does not match the authorization request");
    return;
  }
  var accessToken = "at-" + toHex(ctx.crypto.randomBytes(16));
  ctx.state.set("oauth:token:" + accessToken, { active: true }, { ttl_ms: TOKEN_TTL_MS });
  ctx.respond(
    200,
    {
      "Content-Type": "application/json; charset=utf-8",
      "Cache-Control": "no-store",
      Pragma: "no-cache",
    },
    JSON.stringify({
      access_token: accessToken,
      token_type: "Bearer",
      expires_in: TOKEN_TTL_MS / 1000,
    })
  );
}

if (!verifySignature()) {
  // Signature error already sent.
} else if (!contentTypeIsForm()) {
  sendError(400, "invalid_request", "body must be application/x-www-form-urlencoded");
} else {
  var form = parseForm();
  if (form === null) {
    sendError(400, "invalid_request", "form body must be UTF-8");
  } else if (form === false) {
    sendError(400, "invalid_request", "form body has malformed percent-encoding");
  } else if (form.grant_type !== "authorization_code") {
    sendError(400, "invalid_request", "grant_type must be authorization_code");
  } else if (
    form.client_id === undefined ||
    form.client_secret === undefined ||
    form.code === undefined ||
    form.redirect_uri === undefined
  ) {
    sendError(400, "invalid_request", "missing required parameter");
  } else if (!isLowerHex(form.code, 64) || form.redirect_uri.length === 0) {
    sendError(400, "invalid_request", "malformed parameter");
  } else if (form.client_id !== CLIENT_ID || form.client_secret !== CLIENT_SECRET) {
    sendError(401, "invalid_client", "client_id or client_secret is invalid");
  } else if (form.redirect_uri !== REDIRECT_URI) {
    sendError(400, "invalid_grant", "redirect_uri does not match the authorization request");
  } else {
    exchange(form);
  }
}
