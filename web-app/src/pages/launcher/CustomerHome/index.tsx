import useHomeV3 from "@/hooks/useHomeV3";
import HomeV2 from "./HomeV2";
import HomeV3 from "./HomeV3";

/** The org's home with the apps-first experience on: v3 behind `home_v3`, else v2. */
export default function CustomerHome() {
  return useHomeV3() ? <HomeV3 /> : <HomeV2 />;
}
