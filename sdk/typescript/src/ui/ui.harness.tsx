// Shared by the desk UI's jsdom tests (not itself a test file): the phone kit's harness, so every
// SDK component is mounted and driven one way. Mount for real, act like a person, inside `act`.
// Add helpers here only when two groups need the same one; a group's own helper stays beside its
// tests.

export {
  $,
  $$,
  click,
  fire,
  type Mounted,
  press,
  render,
  stubMatchMedia
} from "../shell/phone/phone.harness";
