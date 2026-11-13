import SortRow from "@/components/SemanticQueryPanel/SortRow";
import type { Field, Order } from "../../../types";

interface SortsSectionProps {
  orders: Order[];
  availableFields: Field[];
  onUpdateOrder: (index: number, updates: Order) => void;
  onRemoveOrder: (index: number) => void;
}

const SortsSection = ({
  orders,
  availableFields,
  onUpdateOrder,
  onRemoveOrder
}: SortsSectionProps) => {
  if (orders.length === 0) return null;

  return (
    <div className='flex w-full flex-col space-y-2 border-b p-3'>
      <p className='font-medium text-muted-foreground text-xs uppercase tracking-wider'>Sorts</p>
      {orders.map((order, index) => (
        <SortRow
          key={`${order.field}-${order.direction}-${index}`}
          order={order}
          availableFields={availableFields}
          onUpdate={(updates) => onUpdateOrder(index, updates)}
          onRemove={() => onRemoveOrder(index)}
        />
      ))}
    </div>
  );
};

export default SortsSection;
