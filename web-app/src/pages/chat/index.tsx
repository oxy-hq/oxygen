import { askPlaceholder } from "@/components/Ask/askPlaceholder";
import ChatPanel from "@/components/Chat/ChatPanel";
import { ThreadHistory } from "@/components/ThreadHistory";
import { getGreeting } from "@/libs/utils/greeting";
import useCurrentOrg from "@/stores/useCurrentOrg";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";

/**
 * The Chat landing — a ChatGPT-like composer with the workspace's recent threads
 * below it. Nothing in the chrome links here since the rail's Chat entry went
 * (#4385); it opens by address, `/threads`. Reuses the same ask-locked composer as
 * the Ask dock (default agent, extended-thinking toggle, no selectors), but
 * WITHOUT `onThreadCreated`, so submitting navigates to the full thread page
 * (`/threads/:id`) rather than streaming in the dock.
 */
const ChatPage = () => {
  const orgName = useCurrentOrg((s) => s.org?.name);
  // Switching workspaces only changes the `:wsId` route param — React Router
  // reuses this same component instance rather than remounting it. Key the
  // panel by workspace id so its typed/generated draft (e.g. SQL) can't
  // survive a workspace switch and leak into the next one (see #2962).
  const wsId = useCurrentWorkspace((s) => s.workspace?.id);
  return (
    <div data-testid='chat-page' className='flex h-full flex-col overflow-auto'>
      <div className='mx-auto w-full max-w-2xl px-6 pt-16 pb-16 sm:pt-24'>
        <p className='mb-6 text-balance text-center text-xl sm:text-2xl'>
          {getGreeting()}! How can I assist you?
        </p>
        <ChatPanel
          key={wsId}
          lockMode='ask'
          hideAgentPicker
          placeholderOverride={askPlaceholder(orgName)}
        />
        <ThreadHistory className='mt-10' />
      </div>
    </div>
  );
};

export default ChatPage;
