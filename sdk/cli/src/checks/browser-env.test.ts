/**
 * The environment the promote checks' browser is started with: what it needs,
 * and no credential.
 */

import { describe, expect, it } from "vitest";
import { browserEnv } from "./browser-env.js";

describe("browserEnv", () => {
  it("keeps what a browser needs to run", () => {
    const needed = {
      PATH: "/usr/bin:/bin",
      HOME: "/home/runner",
      USER: "runner",
      TMPDIR: "/tmp",
      LANG: "en_US.UTF-8",
      LC_ALL: "C.UTF-8",
      LC_CTYPE: "en_US.UTF-8",
      TZ: "UTC",
      DISPLAY: ":99",
      WAYLAND_DISPLAY: "wayland-0",
      XDG_RUNTIME_DIR: "/run/user/1001",
      XDG_CONFIG_HOME: "/home/runner/.config",
      DBUS_SESSION_BUS_ADDRESS: "unix:path=/run/user/1001/bus",
      FONTCONFIG_PATH: "/etc/fonts",
      SSL_CERT_FILE: "/etc/ssl/certs/ca-certificates.crt",
      PLAYWRIGHT_BROWSERS_PATH: "/home/runner/.cache/ms-playwright",
      PW_TEST_SCREENSHOT_NO_FONTS_READY: "1"
    };
    expect(browserEnv(needed)).toEqual(needed);
  });

  it("leaves out every credential: the checker's token under any name, and a CI's own", () => {
    const kept = browserEnv({
      PATH: "/usr/bin",
      OXY_API_KEY: "oxysvc_x",
      OXY_TOKEN: "oxy_pat_x",
      OXY_CHECKS_NO_BEARER: "",
      // What `--api-key-env` or `--token-env` could have named.
      MY_DEPLOY_CREDENTIAL: "oxysvc_y",
      CHECKER: "oxysvc_z",
      OXY_OIDC_TOKEN_FILE: "/var/run/token",
      OXY_SERVICE_ACCOUNT: "sa-1",
      GITHUB_TOKEN: "ghs_x",
      GH_TOKEN: "ghp_x",
      ACTIONS_ID_TOKEN_REQUEST_TOKEN: "jwt",
      ACTIONS_ID_TOKEN_REQUEST_URL: "https://token.actions",
      ACTIONS_RUNTIME_TOKEN: "jwt",
      AWS_SECRET_ACCESS_KEY: "s",
      NPM_TOKEN: "npm_x",
      NODE_OPTIONS: "--require /tmp/x.js"
    });
    expect(kept).toEqual({ PATH: "/usr/bin" });
  });

  it("does not keep a secret because its name starts like a kept one", () => {
    expect(
      browserEnv({
        PLAYWRIGHT_SERVICE_ACCESS_TOKEN: "t",
        PLAYWRIGHT_SERVICE_URL: "wss://x",
        XDG_SESSION_SECRET: "s",
        PW_API_KEY: "k",
        LC_PASSWORD: "p"
      })
    ).toEqual({ PLAYWRIGHT_SERVICE_URL: "wss://x" });
  });

  it("keeps a proxy's address, in either case, but not one that carries a password", () => {
    expect(
      browserEnv({
        HTTPS_PROXY: "http://proxy.internal:3128",
        http_proxy: "http://proxy.internal:3128",
        NO_PROXY: "localhost,127.0.0.1,.internal",
        HTTP_PROXY: "http://user:hunter2@proxy.internal:3128",
        ALL_PROXY: "user:hunter2@proxy.internal:1080"
      })
    ).toEqual({
      HTTPS_PROXY: "http://proxy.internal:3128",
      http_proxy: "http://proxy.internal:3128",
      NO_PROXY: "localhost,127.0.0.1,.internal"
    });
  });

  it("reads names in any case, as Windows spells them, and skips unset ones", () => {
    expect(
      browserEnv({
        Path: "C:\\Windows\\system32",
        SystemRoot: "C:\\Windows",
        LocalAppData: "C:\\Users\\r\\AppData\\Local",
        Oxy_Api_Key: "oxysvc_x",
        TEMP: undefined
      })
    ).toEqual({
      Path: "C:\\Windows\\system32",
      SystemRoot: "C:\\Windows",
      LocalAppData: "C:\\Users\\r\\AppData\\Local"
    });
  });
});
