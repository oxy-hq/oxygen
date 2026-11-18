import type React from "react";
import { useEffect, useState } from "react";
import useApproveCliDevice from "@/hooks/api/cliAuth/useApproveCliDevice";
import useCliDeviceRequest from "@/hooks/api/cliAuth/useCliDeviceRequest";
import useDenyCliDevice from "@/hooks/api/cliAuth/useDenyCliDevice";
import { apiStatus } from "@/libs/apiError";
import { loginUrl } from "../../cliAuthRequest";
import { type CliSession, leaveTo, readCliSession } from "../../cliSession";
import AgentApproval from "../../components/AgentApproval";
import MintApproval, { type ApprovalSubmit } from "../../components/MintApproval";
import MintPage from "../../components/MintPage";
import ServiceAccountApproval from "../../components/ServiceAccountApproval";
import { deviceReturnTo, readDeviceAsk } from "../deviceRequest";
import DeviceLoginApproval from "./DeviceLoginApproval";
import DeviceRequestMissing from "./DeviceRequestMissing";

type Step = "checking" | "confirm" | "approved" | "declined";

/**
 * The browser half of `oxyc … --no-browser`, for one user code. The person signs in if they
 * aren't, the request oxyc started is read back, and it is laid out on the same sheet a link
 * opens — with the code to compare before anything can be approved. Approving records the
 * decision on the request; oxyc, polling, collects the token. Nothing leaves this page for oxyc.
 */
const DeviceAuthorize: React.FC<{ userCode: string }> = ({ userCode }) => {
  const [step, setStep] = useState<Step>("checking");
  const [session, setSession] = useState<CliSession | null>(null);
  const request = useCliDeviceRequest(step === "checking" ? null : userCode);
  const approveDevice = useApproveCliDevice(userCode);
  const deny = useDenyCliDevice(userCode);
  const loginHref = loginUrl(deviceReturnTo(window.location.origin, userCode));

  useEffect(() => {
    let cancelled = false;
    readCliSession()
      .then((found) => {
        if (cancelled) return;
        if (!found) return leaveTo(loginHref);
        setSession(found);
        setStep("confirm");
      })
      .catch(() => {
        if (!cancelled) leaveTo(loginHref);
      });
    return () => {
      cancelled = true;
    };
  }, [loginHref]);

  // The session lapsed between the check and the read: sign in again and come back here.
  useEffect(() => {
    if (apiStatus(request.error) === 401) leaveTo(loginHref);
  }, [request.error, loginHref]);

  if (step === "checking" || (step === "confirm" && request.isPending)) {
    return (
      <MintPage
        status='working'
        title='Checking the request…'
        lead='Making sure you are signed in, and finding what oxyc asked for.'
      />
    );
  }
  if (step === "approved") {
    return (
      <MintPage
        status='done'
        title='Approved'
        lead={`oxyc on ${request.data?.hostname ?? "that computer"} picks it up within a few seconds and carries on. You can close this tab.`}
      />
    );
  }
  if (step === "declined") {
    return (
      <MintPage
        status='error'
        title='Request declined'
        lead={
          deny.isError
            ? "No token was created. oxyc could not be told, so it waits until the request lapses; you can close this tab."
            : "No token was created, and oxyc stops waiting. You can close this tab."
        }
      />
    );
  }
  if (!request.data) {
    return (
      <DeviceRequestMissing
        error={request.error ?? new Error("no request")}
        onRetry={() => request.refetch()}
      />
    );
  }

  // A refused approval (a 4xx) already declined the request, with its reason: oxyc has it.
  const refused = approveDevice.isError && (apiStatus(approveDevice.error) ?? 500) < 500;
  const decline = () => {
    setStep("declined");
    if (!refused) deny.mutate();
  };
  const onApproved = () => setStep("approved");
  const onSessionLapsed = () => leaveTo(loginHref);
  const submit: ApprovalSubmit = {
    pending: approveDevice.isPending,
    send: (mint, { onSuccess, onError }) =>
      approveDevice.mutate({ mint }, { onSuccess: () => onSuccess(null), onError })
  };

  const { hostname } = request.data;
  const ask = readDeviceAsk(request.data.ask);
  const sheet = { email: session?.email, submit, userCode, onApproved, onSessionLapsed };
  switch (ask.kind) {
    case "login":
      return (
        <DeviceLoginApproval
          hostname={hostname}
          userCode={userCode}
          email={session?.email}
          pending={approveDevice.isPending}
          failed={approveDevice.isError && apiStatus(approveDevice.error) !== 401}
          onApprove={() =>
            approveDevice.mutate(
              {},
              {
                onSuccess: onApproved,
                onError: (error) => {
                  if (apiStatus(error) === 401) onSessionLapsed();
                }
              }
            )
          }
          onDecline={decline}
        />
      );
    case "mint":
      return <MintApproval request={{ hostname, ask: ask.ask }} onDeclined={decline} {...sheet} />;
    case "agent_mint":
      return <AgentApproval request={{ hostname, ask: ask.ask }} onDeclined={decline} {...sheet} />;
    case "service_account_mint":
      return (
        <ServiceAccountApproval
          request={{ hostname, ask: ask.ask }}
          onDeclined={decline}
          {...sheet}
        />
      );
    case "invalid":
      return <MintPage status='error' title={ask.title} lead={ask.reason} />;
  }
};

export default DeviceAuthorize;
