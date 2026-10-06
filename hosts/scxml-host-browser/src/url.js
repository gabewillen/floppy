// The URL form the page's allowlists see.
//
// `fetch` parses its URL with the WHATWG URL parser, which removes dot
// segments (`..`, `%2e%2e`, and, for http(s), backslash-separated ones).
// A policy that compared the raw string would accept
// `https://api.example/scxml/../admin` under a `/scxml/` prefix while the
// fetch reaches `/admin`. So the host parses each http(s) URL once, hands
// the page's `allow` the parsed `href`, and passes that same `href` to the
// page's `resolve` or `send` (and to the default `fetch`): the string the
// policy judged is the string that is fetched.

/**
 * The canonical form of `url` (`new URL(url).href`) when it is an absolute
 * `http:` or `https:` URL without userinfo; otherwise `undefined`, and the
 * host denies it without asking the page (fetch refuses userinfo too).
 * @param {unknown} url
 * @returns {string | undefined}
 */
export function canonicalHttpUrl(url) {
  if (typeof url !== 'string') return undefined;
  let parsed;
  try {
    parsed = new URL(url);
  } catch {
    return undefined;
  }
  if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') return undefined;
  if (parsed.username !== '' || parsed.password !== '') return undefined;
  return parsed.href;
}

/**
 * The form of a document URL (`resources`) the page's `allow` and
 * `resolve` get: an http(s) URL's canonical form (`canonicalHttpUrl`;
 * `undefined`, a denial, when it carries userinfo), and any other string
 * exactly as written. The host never fetches a URL itself, and a page may
 * map another scheme (`file:`) or a relative name to its own documents; a
 * policy admits those only by parsing them itself.
 * @param {string} url
 * @returns {string | undefined}
 */
export function resourceUrl(url) {
  let parsed;
  try {
    parsed = new URL(url);
  } catch {
    return url;
  }
  if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') return url;
  return canonicalHttpUrl(url);
}
