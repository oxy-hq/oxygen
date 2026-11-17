/** One order, as the warehouse returns it. */
export interface Order {
  id: string;
  /**
   * In minor units.
   * @asType integer
   */
  totalCents: number;
}
