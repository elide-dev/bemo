plugins { application }

repositories { mavenCentral() }

val bemoVersion = "0.3.0"
val bemoClassifier = providers.gradleProperty("bemo.classifier").getOrElse("linux-x86_64-gnu")

dependencies {
  implementation("dev.elide.bemo:bemo-netty:$bemoVersion")
  implementation("dev.elide.bemo:bemo-ffm:$bemoVersion")
  runtimeOnly("dev.elide.bemo:bemo-ffm:$bemoVersion:$bemoClassifier")
}

tasks.withType<JavaCompile>().configureEach { options.release.set(22) }

application {
  mainClass.set("dev.elide.bemo.examples.quickstart.Application")
  applicationDefaultJvmArgs = listOf("--enable-native-access=ALL-UNNAMED")
}
