// GET /demo/documents/manifest/:group — plans/demo-document-catalog.md §3.1.
//
// The :group path parameter is kept for existing callers and does not filter
// the metadata: every request returns all records of the upstream data array.
var HEADER = "文件编码,文件标题,系统代码";
var METADATA_URL =
  ctx.env.METADATA_API_URL || "https://metadata.example.com/demo/documents";

// CSV rule: quote a field that contains a comma, a double quote, CR or LF,
// and double every internal double quote.
function csvField(value) {
  var text = value === undefined || value === null ? "" : String(value);
  if (
    text.indexOf(",") !== -1 ||
    text.indexOf('"') !== -1 ||
    text.indexOf("\n") !== -1 ||
    text.indexOf("\r") !== -1
  ) {
    return '"' + text.split('"').join('""') + '"';
  }
  return text;
}

// Metadata failures answer 502 metadata_bad_gateway; the diagnostic reason
// goes to the server log only, never to the client body.
function metadataBadGateway(reason) {
  ctx.log.error("metadata_bad_gateway: " + reason);
  ctx.respond(
    502,
    { "Content-Type": "application/json; charset=utf-8" },
    JSON.stringify({ error: "metadata_bad_gateway" })
  );
}

function manifest() {
  var metadata;
  try {
    metadata = ctx.http.get(METADATA_URL);
  } catch (error) {
    // A rejected call (allowlist, URL policy) is a configuration error and
    // stays a script error; only transport failures are gateway failures.
    if (error.code !== "upstream_unreachable") {
      throw error;
    }
    metadataBadGateway("metadata upstream unreachable");
    return;
  }
  if (metadata.status < 200 || metadata.status > 299) {
    metadataBadGateway("metadata upstream returned " + metadata.status);
    return;
  }
  var payload;
  try {
    payload = JSON.parse(metadata.text());
  } catch (error) {
    metadataBadGateway("metadata response is not valid JSON");
    return;
  }
  if (!payload || !Array.isArray(payload.data)) {
    metadataBadGateway("metadata response has no data array");
    return;
  }

  var rows = [HEADER];
  for (var i = 0; i < payload.data.length; i += 1) {
    var item = payload.data[i];
    rows.push(
      [
        csvField(item.code),
        csvField(item.title),
        csvField(item.system_code),
      ].join(",")
    );
  }
  ctx.respond(
    200,
    { "Content-Type": "text/plain; charset=utf-8" },
    rows.join("\n") + "\n"
  );
}

manifest();
