import { toast } from "sonner";
import { Button } from "@/components/ui/shadcn/button";
import { initiateOktaAuth } from "@/hooks/auth/useOktaAuth";

interface Props {
  disabled: boolean;
  clientId: string;
  domain: string;
  /** Where to land after sign-in; absent, the login URL's own `return_to`. */
  returnTo?: string;
}

const LoginWithOktaButton = ({ disabled, clientId, domain, returnTo }: Props) => {
  const handleOktaAuth = async () => {
    try {
      await initiateOktaAuth(clientId, domain, returnTo);
    } catch (err) {
      console.error("Failed to start Okta login:", err);
      toast.error("Couldn't start Okta login. Please try again.");
    }
  };

  return (
    <Button type='button' className='w-full' onClick={handleOktaAuth} disabled={disabled}>
      <svg
        xmlns='http://www.w3.org/2000/svg'
        viewBox='0 0 24 24'
        className='mr-2 h-4 w-4'
        fill='currentColor'
      >
        <path d='M11.999 2.665c-5.158 0-9.333 4.175-9.333 9.333 0 5.159 4.175 9.334 9.333 9.334 5.159 0 9.334-4.175 9.334-9.334 0-5.158-4.175-9.333-9.334-9.333zm0 14.777c-3.007 0-5.444-2.437-5.444-5.444s2.437-5.443 5.444-5.443c3.008 0 5.445 2.436 5.445 5.443s-2.437 5.444-5.445 5.444z' />
      </svg>
      Login with Okta
    </Button>
  );
};

export default LoginWithOktaButton;
