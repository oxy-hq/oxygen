/**
 * The main step: install `oxyc`, sign it in, and hand the credential to the
 * rest of the job.
 *
 * ORDER MATTERS in three places, and each is a property rather than a habit:
 *
 *   1. Inputs are validated before anything is spawned. They are written into
 *      an `npm install` command line, and on a Windows runner that goes through
 *      a shell.
 *   2. The token is masked the moment it is parsed (`oxy.mjs`), before it is
 *      written anywhere — a mask registered after a value was printed redacts
 *      nothing.
 *   3. The token is saved for the post step BEFORE it is exported. If the
 *      export throws, the token still gets revoked.
 */

import { join } from "node:path";
import {
  addPath,
  error,
  exportVariable,
  getInput,
  info,
  saveState,
  setOutput,
  warning
} from "./io.mjs";
import { exchange, requireOidc, SetupError } from "./oxy.mjs";

/** @typedef {import("./io.mjs").Io} Io */

const PACKAGE = "@oxy-hq/cli";

/** Hosts a token may be sent to over plain `http`: a deployment on this runner. */
const LOOPBACK = new Set(["localhost", "127.0.0.1", "[::1]"]);

/**
 * @typedef {object} Inputs
 * @property {string} version
 * @property {string} serviceAccount
 * @property {string} host No trailing slash.
 * @property {boolean} exportToken
 */

/** The deployment's base URL, normalised — or the reason it is not one. */
function parseHost(/** @type {string} */ raw) {
  /** @type {URL} */
  let url;
  try {
    url = new URL(raw);
  } catch {
    throw new SetupError(`\`host\` is not a URL: ${JSON.stringify(raw)}`, [
      "e.g. host: https://app.oxygen-hq.com"
    ]);
  }
  if (url.username || url.password || url.search || url.hash) {
    throw new SetupError("`host` must be a bare base URL", [
      "no credentials, query string or fragment — e.g. host: https://app.oxygen-hq.com"
    ]);
  }
  const loopback = LOOPBACK.has(url.hostname);
  if (url.protocol !== "https:" && !(url.protocol === "http:" && loopback)) {
    throw new SetupError("`host` must be an https URL", [
      "the token is sent to it. Plain http is accepted for localhost only."
    ]);
  }
  return `${url.origin}${url.pathname.replace(/\/+$/, "")}`;
}

const WHERE_THE_ID_IS =
  "It is shown in the web app under Organization settings → API access → Service accounts → (the account), and `oxyc init-ci` writes it into the workflow.";

/** Read and validate the four inputs. @returns {Inputs} */
export function readInputs(/** @type {Io} */ io) {
  const version = getInput(io, "version") || "latest";
  // An exact version or a dist-tag. Nothing a shell would read as syntax.
  if (!/^[0-9A-Za-z][0-9A-Za-z.+-]*$/.test(version)) {
    throw new SetupError(`\`version\` is not a version or a dist-tag: ${JSON.stringify(version)}`, [
      "e.g. version: 0.6.0, or version: latest"
    ]);
  }

  const serviceAccount = getInput(io, "service-account");
  if (!serviceAccount) {
    // Refused here, before anything is installed or any token is asked for:
    // the deployment answers a request that names no account with a 400.
    throw new SetupError("`service-account` is required", [
      `name the service account whose trust policy matches this workflow, by its ID. ${WHERE_THE_ID_IS}`,
      "the deployment never picks an account on a run's behalf: anyone can register a trust policy that names a repository."
    ]);
  }
  // The ID and nothing else — refused here, before any token is asked for,
  // because the deployment refuses it too. `acme/deployer` is the form that
  // can be re-pointed: a slug is free for anyone once its org renames.
  if (!/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(serviceAccount)) {
    throw new SetupError(
      `\`service-account\` is not a service account ID: ${JSON.stringify(serviceAccount)}`,
      [
        `it takes the account's ID (a UUID), never <org-slug>/<name>: a name can be taken over by another organization, an ID cannot. ${WHERE_THE_ID_IS}`
      ]
    );
  }

  const exportRaw = (getInput(io, "export-token") || "true").toLowerCase();
  if (exportRaw !== "true" && exportRaw !== "false") {
    throw new SetupError(
      `\`export-token\` must be true or false, not ${JSON.stringify(exportRaw)}`
    );
  }

  return {
    version,
    serviceAccount,
    host: parseHost(getInput(io, "host") || "https://app.oxygen-hq.com"),
    exportToken: exportRaw === "true"
  };
}

/** The last few lines of a command's stderr, for an error's hints. */
function tail(/** @type {string} */ text) {
  return text
    .split("\n")
    .map((line) => line.trimEnd())
    .filter(Boolean)
    .slice(-5);
}

/**
 * `npm install` the CLI into a prefix of its own and put it on `PATH`.
 *
 * `--ignore-scripts`: the job that runs this action usually holds
 * `id-token: write`, so nothing fetched from the registry gets to run an
 * install script in it. `oxyc` needs none.
 */
export function install(/** @type {Io} */ io, /** @type {string} */ version) {
  const runnerTemp = io.env.RUNNER_TEMP;
  if (!runnerTemp) {
    throw new SetupError("RUNNER_TEMP is not set — is this running inside GitHub Actions?");
  }
  const prefix = join(runnerTemp, "oxyc");
  const spec = `${PACKAGE}@${version}`;
  const installed = io.exec("npm", [
    "install",
    "--prefix",
    prefix,
    "--no-audit",
    "--no-fund",
    "--ignore-scripts",
    spec
  ]);
  if (installed.status !== 0) {
    throw new SetupError(`npm could not install ${spec}`, [
      ...tail(installed.stderr),
      "the job needs Node.js and npm on PATH — add actions/setup-node before this step."
    ]);
  }

  const bin = join(prefix, "node_modules", ".bin");
  addPath(io, bin);
  const probe = io.exec(join(bin, "oxyc"), ["--version"]);
  if (probe.status !== 0) {
    throw new SetupError(`${spec} was installed but \`oxyc --version\` does not run`, [
      ...tail(probe.stderr),
      "oxyc needs Node.js 20 or newer."
    ]);
  }
  info(io, `installed oxyc ${probe.stdout.trim()}`);
}

/** The whole main step. Throws `SetupError` for anything worth failing the job over. */
export async function setup(/** @type {Io} */ io) {
  const inputs = readInputs(io);
  // Before the install, so a job missing the permission fails in a second
  // rather than after an `npm install`.
  requireOidc(io.env);

  install(io, inputs.version);

  const result = await exchange(io, inputs.host, inputs.serviceAccount);
  if (result.kind === "unsupported") {
    // Not a failure: nothing in the workflow can fix an older deployment, and
    // `oxyc publish` has a way in that does not need this exchange.
    warning(
      io,
      `${inputs.host} has no OIDC token exchange (POST /api/auth/oidc/exchange answered 404): it predates trusted access. ` +
        "oxyc is installed, but no OXY_TOKEN was exported. `oxyc publish` and `oxyc checks run` still authenticate on their own, " +
        "through the app's registered publisher; any other command needs OXY_TOKEN set from a secret."
    );
    return;
  }

  // First, so the post step can revoke it whatever happens below.
  saveState(io, "token", result.token);
  saveState(io, "host", inputs.host);

  if (inputs.exportToken) exportVariable(io, "OXY_TOKEN", result.token);
  setOutput(io, "token", result.token);
  setOutput(io, "token-id", result.tokenId);
  setOutput(io, "expires-at", result.expiresAt);
  setOutput(io, "service-account", result.serviceAccount);

  const who = result.serviceAccount ? ` as ${result.serviceAccount}` : "";
  const until = result.expiresAt
    ? ` It expires at ${result.expiresAt}`
    : " It expires in fifteen minutes";
  info(
    io,
    `signed in to ${inputs.host}${who}.${until}, and is revoked when the job ends.` +
      (inputs.exportToken ? " Exported as OXY_TOKEN for the steps that follow." : "")
  );
}

/** Report a failure as one annotation carrying the fix, and fail the step. */
export function fail(/** @type {Io} */ io, /** @type {unknown} */ cause) {
  const message = cause instanceof Error ? cause.message : String(cause);
  const hints = cause instanceof SetupError ? cause.hints : [];
  error(io, [message, ...hints].join("\n"));
  process.exitCode = 1;
}
