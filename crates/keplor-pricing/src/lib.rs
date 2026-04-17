//! Cost accounting.  Loads the LiteLLM
//! `model_prices_and_context_window.json` catalogue and computes cost in
//! int64 nanodollars, correctly attributing cached / reasoning / batch /
//! tier / geo multipliers.  Built in phase 2.
