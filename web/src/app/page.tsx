import { JudgeDoor } from "@/ui/JudgeDoor";

export const dynamic = "force-dynamic";

export default function Home() {
  return <JudgeDoor hosted={process.env.PRU_HOSTED_DOOR === "1"} />;
}
