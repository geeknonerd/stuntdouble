// POST /demo/documents/upload — plans/demo-document-catalog.md §3.5.
//
// The host parses the multipart body into request-scoped temporary storage
// before this script runs; ctx.request.files exposes each file's metadata and
// contents, and non-file form fields are ignored. The demo expects at least one
// file part, answers the first one's metadata, and persists nothing: the upload
// never reaches the static file root, and the temporary storage is removed when
// the request ends.
var file = ctx.request.files[0];

ctx.respond(
  201,
  { "Content-Type": "application/json; charset=utf-8" },
  JSON.stringify({
    field: file.field,
    filename: file.filename,
    content_type: file.contentType,
    size: file.size,
  })
);
