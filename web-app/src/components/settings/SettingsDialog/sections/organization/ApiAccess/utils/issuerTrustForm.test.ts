import { describe, expect, it } from "vitest";
import {
  audienceFor,
  awsRoleOf,
  buildClientSnippet,
  buildCreateIssuerRequest,
  buildCreateTrustPolicyRequest,
  conditionRow,
  conditionsError,
  conditionsOfForm,
  conditionValueError,
  emptyIssuerForm,
  emptyTrustPolicyForm,
  parseJwks,
  roleArnError,
  validateIssuerForm,
  validateTrustPolicyForm
} from "./issuerTrustForm";

const ROLE = "arn:aws:iam::123456789012:role/ferret-task";
const KEYS = JSON.stringify({ keys: [{ kty: "RSA", kid: "k", n: "abc", e: "AQAB" }] });

describe("claim conditions", () => {
  it("takes an exact value or a prefix with one trailing '*'", () => {
    for (const good of ["exact", "refs/heads/*", "a*"]) {
      expect(conditionValueError(good)).toBeNull();
    }
    for (const bad of ["", "*", " * ", "refs/*/main", "*main", "a**"]) {
      expect(conditionValueError(bad)).not.toBeNull();
    }
  });

  it("refuses 'iss', a claim twice, and a policy on the audience alone", () => {
    expect(conditionsError([{ claim: "sub", value: "x" }])).toBeNull();
    expect(conditionsError([{ claim: "aud", value: "oxy:app.example.com" }])).toMatch(
      /other than 'aud'/
    );
    expect(conditionsError([])).not.toBeNull();
    expect(
      conditionsError([
        { claim: "iss", value: "https://x" },
        { claim: "sub", value: "y" }
      ])
    ).toMatch(/'iss'/);
    expect(
      conditionsError([
        { claim: "sub", value: "a" },
        { claim: "sub", value: "b" }
      ])
    ).toMatch(/twice/);
    expect(conditionsError([{ claim: "jti", value: "x" }])).not.toBeNull();
  });
});

describe("the AWS preset", () => {
  it("names one IAM role by its ARN", () => {
    expect(roleArnError(ROLE)).toBeNull();
    expect(roleArnError("arn:aws-us-gov:iam::123456789012:role/path/ferret")).toBeNull();
    for (const bad of [
      "",
      "ferret-task",
      "arn:aws:iam::123:role/ferret",
      "arn:aws:iam::123456789012:user/ferret",
      "arn:aws:sts::123456789012:assumed-role/ferret/session"
    ]) {
      expect(roleArnError(bad)).not.toBeNull();
    }
  });

  it("writes the role as the one 'sub' condition, and reads it back", () => {
    const form = { ...emptyTrustPolicyForm("aws", "iss-1", "sa-1"), roleArn: ` ${ROLE} ` };
    expect(conditionsOfForm(form)).toEqual([{ claim: "sub", value: ROLE }]);
    expect(validateTrustPolicyForm(form, { mode: "create" })).toEqual({});
    expect(buildCreateTrustPolicyRequest(form, "member")).toEqual({
      issuer_id: "iss-1",
      conditions: [{ claim: "sub", value: ROLE }],
      grants: [{ kind: "workspace", workspace_id: null, role_ceiling: "member" }]
    });
    expect(awsRoleOf([{ claim: "sub", value: ROLE }])).toBe(ROLE);
    expect(
      awsRoleOf([{ claim: "sub", value: "arn:aws:iam::123456789012:role/ferret-*" }])
    ).toBeNull();
    expect(
      awsRoleOf([
        { claim: "sub", value: ROLE },
        { claim: "aud", value: "x" }
      ])
    ).toBeNull();
  });

  it("registers with keys through discovery and single-use tokens", () => {
    const form = {
      ...emptyIssuerForm("aws"),
      name: "oxy-management",
      issuer: "https://abc123.tokens.sts.global.api.aws"
    };
    expect(validateIssuerForm(form)).toEqual({});
    expect(buildCreateIssuerRequest(form)).toEqual({
      name: "oxy-management",
      issuer: "https://abc123.tokens.sts.global.api.aws",
      key_source: "discovery",
      jti_single_use: true,
      preset: "aws"
    });
  });

  it("gives the workload the STS call and the exchange, for the deployment's audience", () => {
    const snippet = buildClientSnippet({
      accountId: "sa-1",
      orgSlug: "acme",
      accountName: "ferret",
      origin: "https://app.example.com"
    });
    expect(audienceFor("https://app.example.com")).toBe("oxy:app.example.com");
    expect(snippet).toContain("--audience 'oxy:app.example.com'");
    expect(snippet).toContain("--signing-algorithm RS256");
    expect(snippet).toContain("https://app.example.com/api/auth/oidc/exchange");
    expect(snippet).toContain('\\"service_account\\": \\"sa-1\\"');
    expect(snippet).toContain("acme/ferret");
    expect(snippet).toContain("OXY_OIDC_TOKEN_FILE=/tmp/oxy-token");
  });
});

describe("a custom issuer", () => {
  it("sends its conditions, skipping empty rows", () => {
    const form = {
      ...emptyTrustPolicyForm("custom", "iss-1", "sa-1"),
      conditions: [
        conditionRow({ claim: " sub ", value: " project_path:acme/app:* " }),
        conditionRow()
      ]
    };
    expect(conditionsOfForm(form)).toEqual([{ claim: "sub", value: "project_path:acme/app:*" }]);
    expect(validateTrustPolicyForm(form, { mode: "create" })).toEqual({});
    const bare = { ...form, conditions: [conditionRow({ claim: "sub", value: "*" })] };
    expect(validateTrustPolicyForm(bare, { mode: "create" }).conditions).toBeDefined();
  });

  it("takes keys pasted inline, public ones only, and sends them parsed", () => {
    const form = {
      ...emptyIssuerForm("custom"),
      name: "build-farm",
      issuer: "https://oidc.build-farm.internal",
      keySource: "inline" as const,
      jwksText: KEYS
    };
    expect(validateIssuerForm(form)).toEqual({});
    expect(buildCreateIssuerRequest(form)).toEqual({
      name: "build-farm",
      issuer: "https://oidc.build-farm.internal",
      key_source: "inline",
      jwks: JSON.parse(KEYS),
      jti_single_use: true,
      preset: "custom"
    });
    expect(parseJwks("not json").error).toBeDefined();
    expect(parseJwks('{"keys": []}').error).toBeDefined();
    expect(parseJwks('{"keys": [{"kty": "RSA", "d": "secret"}]}').error).toMatch(/private/);
  });

  it("takes a key set URL, and a discovery URL only when given", () => {
    const base = { ...emptyIssuerForm("custom"), name: "x", issuer: "https://issuer.example.com" };
    expect(validateIssuerForm({ ...base, keySource: "jwks_uri" }).jwksUri).toBeDefined();
    const jwks = {
      ...base,
      keySource: "jwks_uri" as const,
      jwksUri: "https://issuer.example.com/keys"
    };
    expect(buildCreateIssuerRequest(jwks)).toMatchObject({
      key_source: "jwks_uri",
      jwks_uri: "https://issuer.example.com/keys"
    });
    expect(buildCreateIssuerRequest(base)).not.toHaveProperty("discovery_url");
    const mirrored = { ...base, discoveryUrl: "https://mirror.example.com" };
    expect(buildCreateIssuerRequest(mirrored)).toMatchObject({
      discovery_url: "https://mirror.example.com"
    });
  });
});
