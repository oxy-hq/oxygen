import useTheme from "@/stores/useTheme";

/** The Oxygen Nexus brand mark for the icon rail. Swaps between the
 *  light/dark favicon assets so it reads correctly on the sidebar in either
 *  theme (the assets are fixed-color, not currentColor). Decorative — the rail
 *  button supplies the "Oxygen Nexus" aria-label. */
export function OxygenFactoryMark({ className }: { className?: string }) {
  const theme = useTheme((s) => s.theme);
  const src = theme === "dark" ? "/oxygen-factory-mark-dark.svg" : "/oxygen-factory-mark-light.svg";
  return <img src={src} alt='' className={className} />;
}
