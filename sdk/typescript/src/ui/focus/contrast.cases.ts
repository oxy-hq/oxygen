// The focus group's contrast cases. The group draws one thing: nothing. A place focus is held
// after Try again (a loading line, the start of what a re-read drew) takes no ring on purpose
// (focus.css), and the modal trap and the scroll stop draw with the rings the recipes already
// measure (`focusRing`, src/ui/contrast.cases.ts). The scroll box's ring, which DataSheet draws,
// is the tables group's case.

import type { UiContrastCase } from "../contrast.harness";

export const contrastCases: UiContrastCase[] = [];
