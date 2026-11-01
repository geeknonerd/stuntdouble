// Fallback route script for unmapped paths (404) and unsupported methods (405).
// The host automatically injects the Allow header for 405 responses; the script
// must never set Allow itself.

var kind = ctx.match.kind;

if (kind === "method_not_allowed") {
  ctx.respond(
    405,
    {
      "Content-Type": "application/json; charset=utf-8",
      "Cache-Control": "no-store",
      Pragma: "no-cache",
    },
    JSON.stringify({
      error: "invalid_method",
      error_description: "method not allowed",
    })
  );
} else {
  ctx.respond(
    404,
    {
      "Content-Type": "application/json; charset=utf-8",
      "Cache-Control": "no-store",
      Pragma: "no-cache",
    },
    JSON.stringify({
      error: "not_found",
      error_description: "path not found",
    })
  );
}
