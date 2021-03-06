import setuptools

with open("README.md") as f:
    long_description = f.read()

with open('requirements.txt') as f:
    data = f.read()

requirements = [line.strip() for line in data.split('\n') if line.strip() != '']

setuptools.setup(
    name="wsrpc",
    version="1.0.0",
    author="Raphael Bernhard",
    author_email="beraphae@gmail.com",
    description="WebSocket & HTTP RPC library",
    long_description=long_description,
    long_description_content_type="text/markdown",
    url="https://github.com/raffber/wsrpc.git",
    packages=['pywsrpc'],
    classifiers=[
        "Programming Language :: Python :: 3",
        "License :: OSI Approved :: MIT License",
        "Operating System :: OS Independent",
    ],
    python_requires='>=3.6',
    install_requires=requirements,
)

