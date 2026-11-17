import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { BrowserRouter } from "react-router-dom";
import { exchangeBrowserTicket } from "./api/client";
import App from "./App";
import "./styles/tokens.css";
import "./styles/globals.css";

const queryClient = new QueryClient();
void exchangeBrowserTicket().catch(() => undefined);

const root = document.getElementById("root");
if (!root) throw new Error("root element not found");
createRoot(root).render(
  <StrictMode>
    <QueryClientProvider client={queryClient}>
      <BrowserRouter>
        <App />
      </BrowserRouter>
    </QueryClientProvider>
  </StrictMode>,
);
