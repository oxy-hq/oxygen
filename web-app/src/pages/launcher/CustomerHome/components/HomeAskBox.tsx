import { ArrowUp } from "lucide-react";
import { type FormEvent, useState } from "react";
import { askPlaceholder } from "@/components/Ask/askPlaceholder";
import { OxyMark } from "@/components/OxyMark";
import {
  InputGroup,
  InputGroupAddon,
  InputGroupButton,
  InputGroupInput
} from "@/components/ui/shadcn/input-group";
import useAskDock from "@/stores/useAskDock";

/**
 * The home's ask box. It is not a second chat: submitting hands the question to the
 * Ask dock (the same composer ⌘K opens), which sends it and shows the answer beside
 * the page. An empty submit just opens the dock.
 */
export function HomeAskBox({ orgName }: { orgName?: string }) {
  const [question, setQuestion] = useState("");
  const openAsk = useAskDock((s) => s.open);

  const submit = (e: FormEvent) => {
    e.preventDefault();
    const message = question.trim();
    if (message) {
      openAsk({ message, autoSubmit: true });
      setQuestion("");
    } else {
      openAsk();
    }
  };

  return (
    <form onSubmit={submit} data-testid='home-ask-box'>
      {/* The page's one primary action, so its button takes the brand fill. */}
      <InputGroup className='h-14 rounded-lg border-border bg-card shadow-none dark:bg-card'>
        <InputGroupAddon className='pl-4'>
          <OxyMark className='size-5 text-foreground' />
        </InputGroupAddon>
        <InputGroupInput
          value={question}
          onChange={(e) => setQuestion(e.target.value)}
          placeholder={askPlaceholder(orgName)}
          aria-label='Ask a question'
          autoComplete='off'
          className='text-base md:text-base'
          data-testid='home-ask-input'
        />
        <InputGroupAddon align='inline-end' className='pr-3'>
          <InputGroupButton
            type='submit'
            variant='default'
            size='icon-sm'
            className='size-9 rounded-md bg-brand text-white hover:bg-brand-hover'
            aria-label='Ask'
            data-testid='home-ask-submit'
          >
            <ArrowUp />
          </InputGroupButton>
        </InputGroupAddon>
      </InputGroup>
    </form>
  );
}
