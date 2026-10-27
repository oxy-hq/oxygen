import type { UseFormRegister } from "react-hook-form";
import type { Grain } from "@/pages/ide/Experiments/grain";
import type { CandidateField, PriceFormValues } from "../priceForm";

/** What a candidate's field set needs: which row it is, how to register, and a field's message. */
export interface FieldsProps {
  index: number;
  register: UseFormRegister<PriceFormValues>;
  messageFor: (field: CandidateField) => string | undefined;
  /** `design.grain`: lengths are counted in it, and a first date must start one. */
  grain: Grain;
}
