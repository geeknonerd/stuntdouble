// GET /demo/documents/local-download/:document_id —
// plans/demo-document-catalog.md §3.4.
//
// Offline twin of scripts/download.js: the metadata comes from the fixture in
// the static file root, and the PDF streams from that root through
// ctx.file.stream, so the bytes never enter the script heap. The host owns
// Accept-Ranges, Content-Length, and Content-Range: a valid single Range
// answers 206, an unusable one answers 416.
//
// tradeoff: apiVersion 1 scripts cannot import each other, so the document
// lookup and JSON error shape are duplicated from scripts/download.js; share a
// module when the runtime gains imports.
var METADATA_PATH = "metadata.json";

function sendJson(status, code) {
  ctx.respond(
    status,
    { "Content-Type": "application/json; charset=utf-8" },
    JSON.stringify({ error: code })
  );
}

function download() {
  // A missing, unreadable, or malformed fixture is a broken demo, not a
  // client error: the file error or JSON error stays an ordinary script error
  // and answers 500 script_error.
  var payload = JSON.parse(ctx.file.readText(METADATA_PATH));
  if (!payload || !Array.isArray(payload.data)) {
    throw new Error("metadata.json has no data array");
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

  // `file` is a path inside the static file root; absolute paths, `..`
  // components, and targets outside the root are rejected by the host and
  // surface as catchable file_path_invalid errors.
  ctx.respond(
    200,
    {
      "Content-Type": "application/pdf",
      "Content-Disposition":
        'attachment;filename="' + String(item.code) + '.pdf"',
    },
    ctx.file.stream(item.file)
  );
}

download();
