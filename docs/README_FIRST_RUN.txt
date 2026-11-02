RoomLab (beta) — first run
==========================

1. Run RoomLab.exe.
   - Windows 11 already includes the WebView2 runtime RoomLab needs. If it is missing, RoomLab
     shows a message with the Microsoft download link and exits.
2. Choose "Explore Demo". This always works: three scripted agents (labelled DEMO) in a Commons
   room. No model, no internet, nothing is downloaded.
3. Try the other rooms: left panel "+" -> Guild / Story / Crucible. Or open the run menu (top-left)
   -> Scenario pack for ready-made experiments.
4. Optional real model: start LM Studio's server (Developer tab -> Start Server) or Ollama, then
   Settings -> Inference -> Test /models -> pick a model -> Save. Thinking models (Gemma 4, Qwen3)
   need "Reasoning allowance" 800-1500 or they may return no visible text.
   Then edit an agent (click it -> Edit) and set Cognition to L1/L2.

Where is my data?  data\ next to RoomLab.exe (or %LOCALAPPDATA%\RoomLab if this folder is
read-only). Exports go to exports\, logs to logs\. Nothing runs in the background after you close
the window. Deleting this folder removes RoomLab completely.

Honesty notes: DEMO agents are scripted, not AI. SOMA reaction lanes and confidence labels are
expressive conventions, not emotion detection. No model weights or inference engines are included.
See docs\USER_GUIDE.md and docs\KNOWN_LIMITATIONS.md.
