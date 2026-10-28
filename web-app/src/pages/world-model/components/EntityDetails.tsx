import type { WmSelection, WorldModel, WorldModelEntity } from "@/types/worldModel";

const labelOf = (item: { name: string; label?: string | null }) => item.label || item.name;

function EntityButton({
  entity,
  onSelect
}: {
  entity: WorldModelEntity;
  onSelect: (entityId: string) => void;
}) {
  return (
    <button
      type='button'
      onClick={() => onSelect(entity.id)}
      className='w-full rounded-md px-2 py-1.5 text-left text-sm transition-colors hover:bg-accent'
    >
      {entity.label}
    </button>
  );
}

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section className='space-y-1.5'>
      <h3 className='font-medium text-muted-foreground text-xs'>{title}</h3>
      {children}
    </section>
  );
}

/**
 * What the read-only world model says about the selection, in plain words: an
 * entity's description, what it connects to, what is measured about it and the
 * details it carries. Nothing that names a view, a file or an expression.
 */
export function EntityDetails({
  model,
  selection,
  onSelectEntity
}: {
  model: WorldModel;
  selection: WmSelection;
  onSelectEntity: (entityId: string) => void;
}) {
  const byId = new Map(model.entities.map((e) => [e.id, e]));

  if (selection?.kind === "promotion") {
    const fromLabel = byId.get(selection.from)?.label ?? selection.from;
    const toLabel = byId.get(selection.to)?.label ?? selection.to;
    const from = byId.get(selection.from);
    const to = byId.get(selection.to);
    // `functional`: each `from` row points at exactly one `to` row (an order and its
    // store), which is what lets a measure of `from` add up per `to`.
    const functional = model.edges.some(
      (e) => e.from === selection.from && e.to === selection.to && e.functional
    );
    return (
      <div className='space-y-3 p-5' data-testid='world-model-details'>
        <h2 className='font-medium text-sm'>
          {fromLabel} and {toLabel}
        </h2>
        <p className='text-muted-foreground text-sm'>
          {functional
            ? `Each ${fromLabel} belongs to one ${toLabel}, so what is measured about ${fromLabel} also adds up for each ${toLabel}.`
            : `${fromLabel} is connected to ${toLabel}.`}
        </p>
        <div className='flex flex-col'>
          {from && <EntityButton entity={from} onSelect={onSelectEntity} />}
          {to && <EntityButton entity={to} onSelect={onSelectEntity} />}
        </div>
      </div>
    );
  }

  const entity = selection && "entityId" in selection ? byId.get(selection.entityId) : undefined;

  if (!entity) {
    return (
      <div className='space-y-3 p-5' data-testid='world-model-details'>
        <h2 className='font-medium text-sm'>Everything in the map</h2>
        <div className='flex flex-col'>
          {[...byId.values()]
            .sort((a, b) => a.label.localeCompare(b.label))
            .map((e) => (
              <EntityButton key={e.id} entity={e} onSelect={onSelectEntity} />
            ))}
        </div>
      </div>
    );
  }

  const connected = model.edges
    .filter((e) => e.from === entity.id || e.to === entity.id)
    .map((e) => byId.get(e.from === entity.id ? e.to : e.from))
    .filter((e): e is WorldModelEntity => !!e && e.id !== entity.id)
    .filter((e, i, all) => all.findIndex((o) => o.id === e.id) === i);
  const measures = [...entity.own_measures, ...entity.induced_measures];

  return (
    <div className='space-y-5 p-5' data-testid='world-model-details'>
      <div className='space-y-1'>
        <h2 className='font-medium text-base'>{entity.label}</h2>
        {entity.description && (
          <p className='text-muted-foreground text-sm'>{entity.description}</p>
        )}
      </div>
      {connected.length > 0 && (
        <Section title='Connected to'>
          <div className='flex flex-col'>
            {connected.map((e) => (
              <EntityButton key={e.id} entity={e} onSelect={onSelectEntity} />
            ))}
          </div>
        </Section>
      )}
      {measures.length > 0 && (
        <Section title='What is measured'>
          <ul className='space-y-2'>
            {measures.map((m) => (
              <li key={m.name} className='text-sm'>
                <span>{labelOf(m)}</span>
                {m.description && <p className='text-muted-foreground text-xs'>{m.description}</p>}
              </li>
            ))}
          </ul>
        </Section>
      )}
      {entity.dimensions.length > 0 && (
        <Section title='Details kept about each'>
          <p className='text-sm'>{entity.dimensions.map(labelOf).join(", ")}</p>
        </Section>
      )}
    </div>
  );
}
