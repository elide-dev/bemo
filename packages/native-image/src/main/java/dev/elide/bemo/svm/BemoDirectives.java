package dev.elide.bemo.svm;

import java.util.List;
import org.graalvm.nativeimage.c.CContext;

/** Header configuration for the generated Bemo C imports. */
public final class BemoDirectives implements CContext.Directives {
  /** Creates the header configuration for the Native Image builder. */
  public BemoDirectives() {}

  @Override
  public List<String> getHeaderFiles() {
    return List.of("<bemo.h>", "<elide_transport.h>");
  }
}
