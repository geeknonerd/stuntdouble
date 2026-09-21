// GET /demo/documents/download/:document_id — plans/demo-document-catalog.md §3.2.
//
// The metadata entry whose code matches the path parameter owns the PDF URL.
// The PDF body streams straight from the upstream connection to the client
// through ctx.http.pipe, so the script never holds the bytes.
var METADATA_URL =
  ctx.env.METADATA_API_URL || "https://metadata.example.com/demo/documents";

function sendJson(status, code) {
  ctx.respond(
    status,
    { "Content-Type": "application/json; charset=utf-8" },
    JSON.stringify({ error: code })
  );
}

// Metadata failures answer 502 metadata_bad_gateway; the diagnostic reason
// goes to the server log only, never to the client body.
function metadataBadGateway(reason) {
  ctx.log.error("metadata_bad_gateway: " + reason);
  sendJson(502, "metadata_bad_gateway");
}

function download() {
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

  var documentId = ctx.request.params.document_id;
  var item = null;
  for (var i = 0; i < payload.data.length; i += 1) {
    if (String(payload.data[i].code) === documentId) {
      item = payload.data[i];
      break;
    }
  }
  if (item === null) {
    sendJson(404, "document_not_found");
    return;
  }

  if (typeof item.pdf_url !== "string" || item.pdf_url === "") {
    ctx.log.error("pdf_url_invalid: " + documentId + " (missing pdf_url)");
    sendJson(502, "pdf_url_invalid");
    return;
  }

  try {
    // Client Range headers are forwarded by the host; an upstream 206 keeps
    // its Content-Range header on the client response.
    ctx.http.pipe(item.pdf_url, {
      headers: {
        "Content-Type": "application/pdf",
        "Content-Disposition":
          'attachment;filename="' + String(item.code) + '.pdf"',
      },
    });
  } catch (error) {
    // The host rejects a URL that does not parse or is not http/https; that is
    // the metadata's problem and answers the invalid-URL contract.
    if (error.code === "upstream_url_invalid") {
      ctx.log.error("pdf_url_invalid: " + error.message);
      sendJson(502, "pdf_url_invalid");
      return;
    }
    // Transport failures, upstream non-2xx answers, and an unfollowable
    // redirect chain are gateway failures; allowlist rejections keep falling
    // through as script errors per ADR 0005.
    if (
      error.code !== "upstream_unreachable" &&
      error.code !== "upstream_http_error" &&
      error.code !== "upstream_redirect_error"
    ) {
      throw error;
    }
    ctx.log.error("pdf_bad_gateway: " + error.message);
    sendJson(502, "pdf_bad_gateway");
    return;
  }
}

download();
