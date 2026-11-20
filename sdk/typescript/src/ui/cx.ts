// Class joining for `@oxy-hq/sdk/ui`: the shell's own `cx`, exported. One definition: the shell,
// the phone kit and the desk UI join classes the same way, and none of them needs Tailwind's merge
// (every class is a fixed `.oxy-*` name, so no two of them fight over one property by accident).
export { cx } from "../shell/cx";
