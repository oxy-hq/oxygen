// The phone kit: the pieces a custom app's phone view is made of, styled by
// the "Phone kit" section of shell.css. Every app follows one pattern:
//
//   const phone = usePhoneLayout();
//   return phone ? <OrdersPhone {...data} /> : <OrdersDesktop {...data} />;
//
// The desktop component stays as it is; the phone view reuses the same data
// hooks and actions and only changes presentation.

export type { PhoneButtonProps, PhoneButtonTone } from "./PhoneButton";
export { PhoneButton } from "./PhoneButton";
export type { PhoneCardFigure, PhoneCardProps, PhoneCardStatus } from "./PhoneCard";
export { PhoneCard } from "./PhoneCard";
export type { PhoneCardListProps } from "./PhoneCardList";
export { PhoneCardList } from "./PhoneCardList";
export type { PhoneFilterChipProps } from "./PhoneFilterChip";
export { PhoneFilterChip } from "./PhoneFilterChip";
export type { PhoneHintProps } from "./PhoneHint";
export { PhoneHint } from "./PhoneHint";
export type { PhoneKpi, PhoneKpiStripProps } from "./PhoneKpiStrip";
export { PhoneKpiStrip } from "./PhoneKpiStrip";
export type { PhonePageProps } from "./PhonePage";
export { PhonePage } from "./PhonePage";
export type { PhoneRowProps } from "./PhoneRow";
export { PhoneRow } from "./PhoneRow";
export type { PhoneSectionProps } from "./PhoneSection";
export { PhoneSection } from "./PhoneSection";
export type { PhoneSegmentedOption, PhoneSegmentedProps } from "./PhoneSegmented";
export { PhoneSegmented } from "./PhoneSegmented";
export type { PhoneSheetProps } from "./PhoneSheet";
export { PhoneSheet } from "./PhoneSheet";
export type { PhoneStickyActionProps } from "./PhoneStickyAction";
export { PhoneStickyAction } from "./PhoneStickyAction";
export type { PhoneTabBarProps, PhoneTabItem } from "./PhoneTabBar";
export { PhoneTabBar } from "./PhoneTabBar";
export type { PhoneTone } from "./tone";
export { PHONE_LAYOUT_QUERY, usePhoneLayout } from "./usePhoneLayout";
