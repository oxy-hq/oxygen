import { AuthLayout } from "@/components/AuthLayout";
import { Spinner } from "@/components/ui/shadcn/spinner";
import { useKioskDevice } from "@/hooks/auth/useFrontline";
import LoginForm, { KioskLogin } from "./LoginForm";

export default function LoginPage() {
  const { data: device, isPending: isProbingKiosk } = useKioskDevice();

  // On an enrolled kiosk the page is the crew's name board, and it takes the
  // whole screen: a centred column left most of a tablet empty while the
  // names scrolled in a box.
  if (device?.bound) {
    return <KioskLogin device={device} />;
  }

  return (
    <AuthLayout>
      {/* Hold the page until the probe answers. Rendering the account options
          first would flash exactly what a kiosk hides. The probe never throws,
          and for a browser without a kiosk cookie the server runs no query
          before answering. */}
      {isProbingKiosk ? (
        <div className='flex justify-center py-10' data-testid='login-probing'>
          <Spinner />
        </div>
      ) : (
        <LoginForm />
      )}
    </AuthLayout>
  );
}
