// GET /demo/documents/local-manifest/:group —
// plans/demo-document-catalog.md §3.3.
//
// Offline twin of scripts/manifest.js: the same CSV contract, sourced from the
// metadata fixture in the static file root instead of an upstream call. The
// file is read on every request; Stunt Double v1 has no cross-request state.
// The :group path parameter is kept for existing callers and does not filter
// the metadata: every request returns all records of the fixture data array.
//
// tradeoff: apiVersion 1 scripts cannot import each other, so the CSV rules
// are duplicated from scripts/manifest.js; share a module when the runtime
// gains imports.
var HEADER = "文件编码,文件标题,系统代码";
var METADATA_PATH = "metadata.json";

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

function manifest() {
  // A missing, unreadable, or malformed fixture is a broken demo, not a
  // client error: the file error or JSON error stays an ordinary script error
  // and answers 500 script_error.
  var payload = JSON.parse(ctx.file.readText(METADATA_PATH));
  if (!payload || !Array.isArray(payload.data)) {
    throw new Error("metadata.json has no data array");
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
