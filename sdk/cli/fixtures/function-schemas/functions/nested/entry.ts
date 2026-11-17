// A custom `entry`, importing its Input from the parent directory.
import type { Input } from "../shared";

export default async function nested(req: { body: string }) {
  const input = JSON.parse(req.body) as Input;
  return input;
}
