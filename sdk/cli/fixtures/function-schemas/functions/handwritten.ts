// The manifest's own schemas win: `inputSchema` written, `outputSchema: null`.
export interface Input {
  ignored: number;
}

export interface Output {
  ignored: number;
}

export default async function handwritten(): Promise<Output> {
  return { ignored: 1 };
}
