import axios from "axios";
import { Plus } from "lucide-react";
import { useMemo, useState } from "react";
import { toast } from "sonner";
import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import { Button } from "@/components/ui/shadcn/button";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle
} from "@/components/ui/shadcn/dialog";
import { Input } from "@/components/ui/shadcn/input";
import { Label } from "@/components/ui/shadcn/label";
import { Spinner } from "@/components/ui/shadcn/spinner";
import {
  useAdminPrices,
  useProvisionCheckout,
  useProvisionSubscription
} from "@/hooks/api/billing";
import {
  type AdminOrgRow,
  type AdminPriceDto,
  type CheckoutAlreadyPendingError,
  DAYS_UNTIL_DUE_DEFAULT,
  DAYS_UNTIL_DUE_MAX,
  DAYS_UNTIL_DUE_MIN,
  type ProvisionCheckoutResponse,
  type ProvisionItem,
  type ProvisionItemRole,
  type ProvisionSubscriptionResponse
} from "@/services/api/billing";
import PendingCheckoutDialog from "../PendingCheckoutDialog";
import { MethodCard } from "./components/MethodCard";
import { MethodDetailDialog } from "./components/MethodDetailDialog";
import { PriceCard } from "./components/PriceCard";
import { PricePickerDialog } from "./components/PricePickerDialog";
import { METHOD_OPTIONS, type ProvisionMethod } from "./constants";

interface RowState {
  rowId: string;
  price: AdminPriceDto;
  role: ProvisionItemRole;
}

let rowIdCounter = 0;
const newRowId = () => `row-${++rowIdCounter}`;

interface Props {
  org: AdminOrgRow;
  onClose: () => void;
  onSuccess: () => void;
}

export default function ProvisionSubscriptionDialog({ org, onClose, onSuccess }: Props) {
  const { data: prices = [], isLoading, refetch } = useAdminPrices();
  const provisionInvoice = useProvisionSubscription(org.id);
  const provisionCheckout = useProvisionCheckout(org.id);
  const [items, setItems] = useState<RowState[]>([]);
  const [method, setMethod] = useState<ProvisionMethod>("checkout");
  const [daysUntilDue, setDaysUntilDue] = useState<number>(DAYS_UNTIL_DUE_DEFAULT);
  const [detailMethod, setDetailMethod] = useState<ProvisionMethod | null>(null);
  const [pendingInfo, setPendingInfo] = useState<CheckoutAlreadyPendingError | null>(null);
  const [pickerOpen, setPickerOpen] = useState(false);

  const selectedPriceIds = useMemo(() => items.map((i) => i.price.id), [items]);
  const hasSeat = items.some((i) => i.role === "seat");
  const daysUntilDueValid =
    Number.isInteger(daysUntilDue) &&
    daysUntilDue >= DAYS_UNTIL_DUE_MIN &&
    daysUntilDue <= DAYS_UNTIL_DUE_MAX;
  const formValid = items.length > 0 && hasSeat && (method !== "invoice" || daysUntilDueValid);

  const addPrice = (price: AdminPriceDto) => {
    setItems((prev) => [
      ...prev,
      {
        rowId: newRowId(),
        price,
        role: prev.some((i) => i.role === "seat") ? "flat" : "seat"
      }
    ]);
  };

  const removeItem = (rowId: string) => {
    setItems((prev) => {
      const next = prev.filter((i) => i.rowId !== rowId);
      if (next.length > 0 && !next.some((i) => i.role === "seat")) {
        next[0] = { ...next[0], role: "seat" };
      }
      return next;
    });
  };

  const setSeatRow = (rowId: string) => {
    setItems((prev) => prev.map((it) => ({ ...it, role: it.rowId === rowId ? "seat" : "flat" })));
  };

  const buildBody = () => ({
    items: items.map<ProvisionItem>(({ price, role }) => ({ price_id: price.id, role }))
  });

  const submitInvoice = async () => {
    try {
      const res = await provisionInvoice.mutateAsync({
        ...buildBody(),
        days_until_due: daysUntilDue
      });
      reportProvisionInvoiceSuccess(res);
      onSuccess();
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Provisioning failed");
    }
  };

  const submitCheckout = async () => {
    try {
      const res = await provisionCheckout.mutateAsync(buildBody());
      reportCheckoutSuccess(res);
      onSuccess();
    } catch (err) {
      const pending = extractPendingError(err);
      if (pending) {
        setPendingInfo(pending);
        return;
      }
      toast.error(err instanceof Error ? err.message : "Checkout creation failed");
    }
  };

  const onSubmit = async (e: React.FormEvent<HTMLFormElement>) => {
    e.preventDefault();
    if (!formValid) return;
    if (method === "invoice") {
      await submitInvoice();
    } else {
      await submitCheckout();
    }
  };

  const onCheckoutRetry = () => {
    setPendingInfo(null);
    void submitCheckout();
  };

  const submitting = provisionInvoice.isPending || provisionCheckout.isPending;
  const disabled = !formValid || submitting;
  const supportedPrices = useMemo(
    () =>
      prices.filter(
        (p) =>
          p.billing_scheme === "per_unit" ||
          (p.billing_scheme === "tiered" && p.tiers_mode === "graduated")
      ),
    [prices]
  );
  const allPricesAdded = supportedPrices.length > 0 && supportedPrices.length === items.length;

  return (
    <>
      <Dialog open onOpenChange={(open) => !open && onClose()}>
        <DialogContent className={`${DESIGN_REFERENCE} max-w-lg`}>
          <DialogHeader>
            <DialogTitle>Provision subscription — {org.name}</DialogTitle>
          </DialogHeader>

          {isLoading ? (
            <div className='flex items-center gap-2 text-muted-foreground text-xs'>
              <Spinner /> Loading prices…
            </div>
          ) : (
            <form onSubmit={onSubmit} className='space-y-4'>
              <div className='space-y-2'>
                <div className='flex items-center justify-between'>
                  <Label>Prices</Label>
                  <span className='text-muted-foreground text-xs'>
                    Mark exactly one as Seat sync.
                  </span>
                </div>

                {items.length > 0 ? (
                  <div className='space-y-2'>
                    {items.map((item) => (
                      <PriceCard
                        key={item.rowId}
                        price={item.price}
                        isSeatSync={item.role === "seat"}
                        onSeatSyncToggle={() => setSeatRow(item.rowId)}
                        onRemove={() => removeItem(item.rowId)}
                      />
                    ))}
                  </div>
                ) : (
                  <div className=' border border-dashed py-6 text-center text-muted-foreground text-xs'>
                    No prices added yet.
                  </div>
                )}

                <Button
                  type='button'
                  variant='outline'
                  size='sm'
                  onClick={() => setPickerOpen(true)}
                  className='w-full'
                  disabled={supportedPrices.length === 0 || allPricesAdded}
                >
                  <Plus className='mr-1 h-3.5 w-3.5' />
                  {allPricesAdded ? "All prices added" : "Add price"}
                </Button>

                {supportedPrices.length === 0 ? (
                  <div className='space-y-1 pt-1 text-muted-foreground text-xs'>
                    <p>
                      No active recurring per-unit or graduated-tiered prices on the Stripe account.
                    </p>
                    <Button
                      type='button'
                      variant='link'
                      size='sm'
                      onClick={() => refetch()}
                      className='h-auto p-0 text-xs'
                    >
                      Refresh prices from Stripe
                    </Button>
                  </div>
                ) : null}
              </div>

              <div className='space-y-2'>
                <Label>Provisioning method</Label>
                <div className='flex gap-2'>
                  {METHOD_OPTIONS.map((opt) => (
                    <MethodCard
                      key={opt.value}
                      title={opt.title}
                      description={opt.shortDescription}
                      note={opt.note}
                      recommended={opt.recommended}
                      selected={method === opt.value}
                      onSelect={() => setMethod(opt.value)}
                      onShowDetail={() => setDetailMethod(opt.value)}
                    />
                  ))}
                </div>
              </div>

              {method === "invoice" ? (
                <div className='space-y-2'>
                  <div className='flex items-center justify-between'>
                    <Label htmlFor='days-until-due'>Payment terms (days until due)</Label>
                    <span className='text-muted-foreground text-xs'>
                      Min {DAYS_UNTIL_DUE_MIN} — max {DAYS_UNTIL_DUE_MAX}
                    </span>
                  </div>
                  <Input
                    id='days-until-due'
                    type='number'
                    inputMode='numeric'
                    min={DAYS_UNTIL_DUE_MIN}
                    max={DAYS_UNTIL_DUE_MAX}
                    step={1}
                    value={Number.isFinite(daysUntilDue) ? daysUntilDue : ""}
                    aria-invalid={!daysUntilDueValid}
                    onChange={(e) => {
                      const raw = e.target.value;
                      if (raw === "") {
                        setDaysUntilDue(Number.NaN);
                        return;
                      }
                      const n = Number(raw);
                      setDaysUntilDue(Number.isFinite(n) ? Math.trunc(n) : Number.NaN);
                    }}
                  />
                  {!daysUntilDueValid ? (
                    <p className='text-destructive text-xs'>
                      Enter a whole number between {DAYS_UNTIL_DUE_MIN} and {DAYS_UNTIL_DUE_MAX}.
                    </p>
                  ) : null}
                </div>
              ) : null}

              <DialogFooter className='flex-col gap-2 sm:flex-row'>
                <Button type='button' variant='outline' onClick={onClose}>
                  Cancel
                </Button>
                <Button type='submit' disabled={disabled}>
                  {submitting ? "Provisioning…" : "Provision subscription"}
                </Button>
              </DialogFooter>
            </form>
          )}
        </DialogContent>
      </Dialog>

      <PricePickerDialog
        open={pickerOpen}
        prices={supportedPrices}
        excludePriceIds={selectedPriceIds}
        onPick={addPrice}
        onOpenChange={setPickerOpen}
      />

      <MethodDetailDialog method={detailMethod} onClose={() => setDetailMethod(null)} />

      {pendingInfo ? (
        <PendingCheckoutDialog
          orgId={org.id}
          info={pendingInfo}
          onClose={() => setPendingInfo(null)}
          onRecreate={onCheckoutRetry}
        />
      ) : null}
    </>
  );
}

function reportProvisionInvoiceSuccess(res: ProvisionSubscriptionResponse) {
  const inv = res.latest_invoice;
  // Stripe creates the invoice in `draft` status and waits ~1 hour before
  // auto-finalizing + emailing it to the customer (when collection_method is
  // send_invoice). Surface that explicitly so admins don't think the email
  // failed.
  const isDeferredDraft =
    inv?.status === "draft" &&
    inv?.collection_method === "send_invoice" &&
    inv?.auto_advance !== false;

  if (isDeferredDraft) {
    toast.success("Subscription provisioned — invoice will be sent within ~1 hour", {
      description:
        "Stripe finalizes the draft invoice automatically before emailing it. Open the invoice if you want to finalize and send it now.",
      action: inv?.hosted_invoice_url
        ? {
            label: "View invoice",
            onClick: () => window.open(inv.hosted_invoice_url ?? "", "_blank", "noopener")
          }
        : undefined,
      duration: 10_000
    });
    return;
  }

  toast.success("Subscription provisioned", {
    description: inv ? `Invoice ${inv.id} — status: ${inv.status}.` : undefined,
    action:
      inv?.hosted_invoice_url != null
        ? {
            label: "View invoice",
            onClick: () => window.open(inv.hosted_invoice_url ?? "", "_blank", "noopener")
          }
        : undefined
  });
}

function reportCheckoutSuccess(res: ProvisionCheckoutResponse) {
  if (res.email_skipped) {
    toast.warning("Checkout link created — email not sent. Copy the link and share manually.", {
      description: res.email_skip_reason ?? undefined,
      action: {
        label: "Copy",
        onClick: () => {
          void navigator.clipboard.writeText(res.url);
          toast.success("Link copied");
        }
      }
    });
  } else {
    toast.success(`Checkout link sent to ${res.email_sent_to}`);
  }
}

function extractPendingError(err: unknown): CheckoutAlreadyPendingError | null {
  if (!axios.isAxiosError(err) || err.response?.status !== 409) return null;
  const data = err.response.data as { code?: string } | undefined;
  if (data?.code !== "checkout_already_pending") return null;
  return data as CheckoutAlreadyPendingError;
}
