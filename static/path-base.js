/** @type {string | undefined} */
let cached;

/**
 * Prefix where static files are mounted (no trailing slash), e.g. "" or "/vaporslop".
 * Call from main.js / render.js with that file's import.meta (first caller wins).
 */
export function mountBasePath(meta) {
  if (cached !== undefined) return cached;
  const path = new URL(meta.url).pathname;
  const i = path.lastIndexOf("/");
  cached = i <= 0 ? "" : path.slice(0, i);
  return cached;
}
