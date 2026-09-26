import { redirect } from "next/navigation";

export default function Home() {
  // Until sessions exist (M1), the root always lands on the login screen.
  redirect("/login");
}
