/**
 * The environment the promote checks' browser is started with: what a browser
 * needs to run, and nothing that is a credential.
 *
 * The page it opens runs a tenant's JavaScript, and the process that starts
 * it holds the checker's token in a variable (`OXY_API_KEY`, or whichever
 * `--api-key-env` and `--token-env` named). A child process inherits its
 * parent's environment unless it is told otherwise, so the browser is told:
 * an ALLOWLIST, not a list of names to drop, because a variable this code has
 * never heard of (a renamed token, a CI secret) must stay out too.
 *
 * Kept: where programs and files are (`PATH`, `HOME`, the temp directories),
 * the display, the locale, fonts and certificates, Playwright's own settings,
 * and a proxy's address. Dropped: everything else, and anything kept by
 * prefix whose name says it is a secret, and a proxy address that carries a
 * password.
 */

/** Exact names, upper-cased: Windows spells them in any case. */
const KEEP = new Set([
  "PATH",
  "HOME",
  "USER",
  "LOGNAME",
  "SHELL",
  "TMPDIR",
  "TMP",
  "TEMP",
  "LANG",
  "LANGUAGE",
  "TZ",
  "DISPLAY",
  "WAYLAND_DISPLAY",
  "XAUTHORITY",
  "DBUS_SESSION_BUS_ADDRESS",
  "FONTCONFIG_FILE",
  "FONTCONFIG_PATH",
  "LD_LIBRARY_PATH",
  "SSL_CERT_FILE",
  "SSL_CERT_DIR",
  // What a process needs to start at all on Windows.
  "SYSTEMROOT",
  "SYSTEMDRIVE",
  "WINDIR",
  "COMSPEC",
  "PATHEXT",
  "USERPROFILE",
  "APPDATA",
  "LOCALAPPDATA",
  "PROGRAMDATA",
  "PROGRAMFILES",
  "PROGRAMFILES(X86)",
  "HOMEDRIVE",
  "HOMEPATH"
]);

/** Locale categories, the desktop's directories, and Playwright's own settings. */
const KEEP_PREFIXES = ["LC_", "XDG_", "PLAYWRIGHT_", "PW_"];

const PROXIES = new Set(["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "NO_PROXY"]);

/** A name that says its value is a secret, whatever prefix it carries. */
const SECRET_NAME = /TOKEN|SECRET|PASSWORD|PASSWD|CREDENTIAL|API_?KEY|PRIVATE/;

/**
 * Whether a proxy address carries a user or a password. `user:pass@` is the
 * only way one gets in, with a scheme or without, and neither an address nor
 * `NO_PROXY`'s host list has an `@` otherwise.
 */
function carriesCredentials(value: string): boolean {
  return value.includes("@");
}

/** The browser's whole environment, picked from `env`. */
export function browserEnv(env: NodeJS.ProcessEnv): Record<string, string> {
  const kept: Record<string, string> = {};
  for (const [name, value] of Object.entries(env)) {
    if (value === undefined) continue;
    const upper = name.toUpperCase();
    if (PROXIES.has(upper)) {
      if (!carriesCredentials(value)) kept[name] = value;
      continue;
    }
    const listed = KEEP.has(upper) || KEEP_PREFIXES.some((prefix) => upper.startsWith(prefix));
    if (listed && !SECRET_NAME.test(upper)) kept[name] = value;
  }
  return kept;
}
